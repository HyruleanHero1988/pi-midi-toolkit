//! Phrase clips on a sample-accurate clock.
//!
//! Clip events are stored in musical ticks and resolved to an **absolute frame**
//! every block. Nothing here reads a wall clock, so a busy UI thread cannot make a
//! loop late: the worst a stalled UI can do is delay a *launch request*, never the
//! timing of notes already playing.

use crate::transport::{Quantize, Transport};

/// Phrase pad grid (MPK Bank A + Bank B).
pub const MAX_CLIPS: usize = 16;
/// Notes one clip may hold open at once (for clean stop / note-off flush).
const MAX_SLOT_NOTES: usize = 24;

/// What a clip event does. Kept `Copy` so scheduling never allocates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipEventKind {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8 },
}

/// One recorded event at a musical offset from the clip start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipEvent {
    pub tick: u32,
    pub kind: ClipEventKind,
}

/// A recorded phrase. Events must be sorted by tick.
#[derive(Debug, Clone, Default)]
pub struct Clip {
    events: Vec<ClipEvent>,
    length_ticks: u32,
}

impl Clip {
    /// Build a clip, sorting events and clamping the loop length to the content.
    pub fn new(mut events: Vec<ClipEvent>, length_ticks: u32) -> Self {
        events.sort_by_key(|e| e.tick);
        let last = events.last().map(|e| e.tick).unwrap_or(0);
        Self {
            events,
            length_ticks: length_ticks.max(last).max(1),
        }
    }

    pub fn events(&self) -> &[ClipEvent] {
        &self.events
    }

    pub fn length_ticks(&self) -> u32 {
        self.length_ticks
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// One-shot plays to the end; loop repeats until stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    OneShot,
    Loop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Idle,
    /// Waiting for a quantize boundary.
    Queued {
        at: u64,
    },
    Playing {
        origin: u64,
        next_index: usize,
    },
    /// Playing but will stop at a boundary.
    Stopping {
        origin: u64,
        next_index: usize,
        at: u64,
    },
}

/// A phrase pad: the clip plus its playback state.
pub struct ClipSlot {
    clip: Option<Box<Clip>>,
    state: SlotState,
    mode: LaunchMode,
    held: [(u8, u8); MAX_SLOT_NOTES],
    held_len: usize,
}

impl Default for ClipSlot {
    fn default() -> Self {
        Self {
            clip: None,
            state: SlotState::Idle,
            mode: LaunchMode::Loop,
            held: [(0, 0); MAX_SLOT_NOTES],
            held_len: 0,
        }
    }
}

impl ClipSlot {
    pub fn set_clip(&mut self, clip: Option<Clip>) {
        self.clip = clip.map(Box::new);
        self.state = SlotState::Idle;
        self.held_len = 0;
    }

    /// Swap a pre-boxed clip. The audio thread must use this so the previous
    /// allocation is returned, not dropped, inside the callback.
    pub fn swap_boxed(&mut self, clip: Option<Box<Clip>>) -> Option<Box<Clip>> {
        let previous = self.clip.take();
        self.clip = clip;
        self.state = SlotState::Idle;
        self.held_len = 0;
        previous
    }

    /// Move the clip out so its allocation can be freed off the audio thread.
    pub fn take_clip(&mut self) -> Option<Clip> {
        self.state = SlotState::Idle;
        self.held_len = 0;
        self.clip.take().map(|b| *b)
    }

    pub fn clip(&self) -> Option<&Clip> {
        self.clip.as_deref()
    }

    pub fn mode(&self) -> LaunchMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: LaunchMode) {
        self.mode = mode;
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.state, SlotState::Idle)
    }

    pub fn is_playing(&self) -> bool {
        matches!(
            self.state,
            SlotState::Playing { .. } | SlotState::Stopping { .. }
        )
    }

    fn track_note(&mut self, kind: ClipEventKind) {
        match kind {
            ClipEventKind::NoteOn { channel, note, .. } => {
                if self.held_len < MAX_SLOT_NOTES
                    && !self.held[..self.held_len].contains(&(channel, note))
                {
                    self.held[self.held_len] = (channel, note);
                    self.held_len += 1;
                }
            }
            ClipEventKind::NoteOff { channel, note } => {
                if let Some(pos) = self.held[..self.held_len]
                    .iter()
                    .position(|h| *h == (channel, note))
                {
                    self.held.swap(pos, self.held_len - 1);
                    self.held_len -= 1;
                }
            }
        }
    }
}

/// An event resolved to a frame inside the current block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeqEvent {
    /// Frame offset within the block being rendered.
    pub frame: u32,
    pub slot: usize,
    pub kind: ClipEventKind,
}

/// The phrase-pad grid.
pub struct Sequencer {
    slots: Vec<ClipSlot>,
}

impl Default for Sequencer {
    fn default() -> Self {
        Self::new()
    }
}

impl Sequencer {
    pub fn new() -> Self {
        Self {
            slots: (0..MAX_CLIPS).map(|_| ClipSlot::default()).collect(),
        }
    }

    pub fn slot(&self, index: usize) -> Option<&ClipSlot> {
        self.slots.get(index)
    }

    pub fn slot_mut(&mut self, index: usize) -> Option<&mut ClipSlot> {
        self.slots.get_mut(index)
    }

    pub fn playing_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_active()).count()
    }

    /// Queue a launch on the next `quantize` boundary at or after `now`.
    ///
    /// Because the grid is anchored at frame 0, two pads launched anywhere inside
    /// the same bar start together — the UI's timing does not leak into the music.
    pub fn launch(&mut self, index: usize, now: u64, quantize: Quantize, transport: &Transport) {
        let at = transport.next_boundary(now, quantize);
        if let Some(slot) = self.slots.get_mut(index) {
            if slot.clip.as_ref().map(|c| c.is_empty()).unwrap_or(true) {
                return;
            }
            slot.state = SlotState::Queued { at };
        }
    }

    /// Queue a stop. Held notes are flushed when the stop actually happens.
    pub fn stop(&mut self, index: usize, now: u64, quantize: Quantize, transport: &Transport) {
        let at = transport.next_boundary(now, quantize);
        if let Some(slot) = self.slots.get_mut(index) {
            slot.state = match slot.state {
                SlotState::Playing { origin, next_index } => SlotState::Stopping {
                    origin,
                    next_index,
                    at,
                },
                SlotState::Queued { .. } => SlotState::Idle,
                other => other,
            };
        }
    }

    /// Stop everything immediately; caller should flush the returned note-offs.
    pub fn stop_all(&mut self, out: &mut [SeqEvent]) -> usize {
        let mut n = 0;
        for index in 0..self.slots.len() {
            n += self.flush_held(index, 0, &mut out[n..]);
            self.slots[index].state = SlotState::Idle;
        }
        n
    }

    fn flush_held(&mut self, index: usize, frame: u32, out: &mut [SeqEvent]) -> usize {
        let slot = &mut self.slots[index];
        let mut n = 0;
        for i in 0..slot.held_len {
            if n >= out.len() {
                break;
            }
            let (channel, note) = slot.held[i];
            out[n] = SeqEvent {
                frame,
                slot: index,
                kind: ClipEventKind::NoteOff { channel, note },
            };
            n += 1;
        }
        slot.held_len = 0;
        n
    }

    /// Resolve every clip event landing in `[block_start, block_start + frames)`.
    ///
    /// Returns the number of events written to `out`, sorted by frame.
    pub fn collect(
        &mut self,
        transport: &Transport,
        block_start: u64,
        frames: u32,
        out: &mut [SeqEvent],
    ) -> usize {
        let block_end = block_start + frames as u64;
        let spt = transport.samples_per_tick();
        let mut n = 0;

        for index in 0..self.slots.len() {
            if n >= out.len() {
                break;
            }
            // Promote a queued launch once its boundary is inside this block.
            if let SlotState::Queued { at } = self.slots[index].state {
                if at < block_end {
                    self.slots[index].state = SlotState::Playing {
                        origin: at,
                        next_index: 0,
                    };
                } else {
                    continue;
                }
            }

            let (origin, mut next_index, stop_at) = match self.slots[index].state {
                SlotState::Playing { origin, next_index } => (origin, next_index, None),
                SlotState::Stopping {
                    origin,
                    next_index,
                    at,
                } => (origin, next_index, Some(at)),
                _ => continue,
            };

            let length_ticks = match self.slots[index].clip.as_ref() {
                Some(clip) if !clip.is_empty() => clip.length_ticks() as u64,
                _ => {
                    self.slots[index].state = SlotState::Idle;
                    continue;
                }
            };
            let loop_len = (length_ticks as f64 * spt).round().max(1.0) as u64;
            let mode = self.slots[index].mode;

            let mut origin = origin;
            let mut finished = false;

            loop {
                if n >= out.len() {
                    break;
                }
                let event_count = self.slots[index]
                    .clip
                    .as_ref()
                    .map(|c| c.events().len())
                    .unwrap_or(0);

                if next_index >= event_count {
                    if mode == LaunchMode::Loop && stop_at.is_none() {
                        origin += loop_len;
                        next_index = 0;
                        continue;
                    }
                    // One-shot (or stopping) reached the end.
                    let end = origin + loop_len;
                    if end < block_end {
                        let frame = end.saturating_sub(block_start) as u32;
                        n += self.flush_held(index, frame.min(frames), &mut out[n..]);
                        finished = true;
                    }
                    break;
                }

                let event = self.slots[index]
                    .clip
                    .as_ref()
                    .map(|c| c.events()[next_index])
                    .expect("clip present");
                let event_frame = origin + (event.tick as f64 * spt).round() as u64;

                if let Some(at) = stop_at {
                    if event_frame >= at {
                        let frame = at.saturating_sub(block_start).min(frames as u64) as u32;
                        if at < block_end {
                            n += self.flush_held(index, frame, &mut out[n..]);
                            finished = true;
                        }
                        break;
                    }
                }

                if event_frame >= block_end {
                    break;
                }

                let frame = event_frame.saturating_sub(block_start) as u32;
                out[n] = SeqEvent {
                    frame: frame.min(frames.saturating_sub(1)),
                    slot: index,
                    kind: event.kind,
                };
                self.slots[index].track_note(event.kind);
                n += 1;
                next_index += 1;
            }

            self.slots[index].state = if finished {
                SlotState::Idle
            } else {
                match stop_at {
                    Some(at) => SlotState::Stopping {
                        origin,
                        next_index,
                        at,
                    },
                    None => SlotState::Playing { origin, next_index },
                }
            };
        }

        out[..n].sort_by_key(|e| e.frame);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::PPQ;

    fn transport() -> Transport {
        let mut t = Transport::new(48_000.0);
        t.set_bpm(120.0);
        t
    }

    /// Four on the floor: a kick on each beat of one bar.
    fn four_on_the_floor() -> Clip {
        let events = (0..4)
            .map(|beat| ClipEvent {
                tick: beat * PPQ,
                kind: ClipEventKind::NoteOn {
                    channel: 9,
                    note: 36,
                    velocity: 100,
                },
            })
            .collect();
        Clip::new(events, PPQ * 4)
    }

    /// Render `total` frames in `block` sized chunks, returning absolute event frames.
    fn run(seq: &mut Sequencer, t: &Transport, total: u64, block: u32) -> Vec<u64> {
        let mut out = [SeqEvent {
            frame: 0,
            slot: 0,
            kind: ClipEventKind::NoteOff {
                channel: 0,
                note: 0,
            },
        }; 64];
        let mut hits = Vec::new();
        let mut pos = 0u64;
        while pos < total {
            let frames = block.min((total - pos) as u32);
            let n = seq.collect(t, pos, frames, &mut out);
            for ev in &out[..n] {
                if matches!(ev.kind, ClipEventKind::NoteOn { .. }) {
                    hits.push(pos + ev.frame as u64);
                }
            }
            pos += frames as u64;
        }
        hits
    }

    #[test]
    fn events_land_on_exact_frames_regardless_of_block_size() {
        let t = transport();
        let beat = t.samples_per_beat() as u64; // 24_000
        let expected: Vec<u64> = (0..4).map(|i| i * beat).collect();

        // A ragged block size is the interesting case: nothing may snap to it.
        for block in [64u32, 128, 37, 512, 1000] {
            let mut seq = Sequencer::new();
            seq.slot_mut(0).unwrap().set_clip(Some(four_on_the_floor()));
            seq.launch(0, 0, Quantize::Off, &t);
            let hits = run(&mut seq, &t, beat * 4, block);
            assert_eq!(hits, expected, "block size {block} shifted the beat");
        }
    }

    #[test]
    fn loop_does_not_drift_over_many_cycles() {
        let t = transport();
        let beat = t.samples_per_beat() as u64;
        let bar = beat * 4;
        let mut seq = Sequencer::new();
        seq.slot_mut(0).unwrap().set_clip(Some(four_on_the_floor()));
        seq.slot_mut(0).unwrap().set_mode(LaunchMode::Loop);
        seq.launch(0, 0, Quantize::Off, &t);

        let cycles = 32u64;
        let hits = run(&mut seq, &t, bar * cycles, 128);
        assert_eq!(hits.len() as u64, 4 * cycles);
        // The last downbeat must be exactly on the grid — no accumulated error.
        assert_eq!(*hits.last().unwrap(), bar * (cycles - 1) + beat * 3);
    }

    #[test]
    fn one_shot_stops_after_its_length() {
        let t = transport();
        let beat = t.samples_per_beat() as u64;
        let mut seq = Sequencer::new();
        seq.slot_mut(0).unwrap().set_clip(Some(four_on_the_floor()));
        seq.slot_mut(0).unwrap().set_mode(LaunchMode::OneShot);
        seq.launch(0, 0, Quantize::Off, &t);

        let hits = run(&mut seq, &t, beat * 12, 128);
        assert_eq!(hits.len(), 4);
        assert!(!seq.slot(0).unwrap().is_active());
    }

    #[test]
    fn quantized_launch_waits_for_the_bar() {
        let t = transport();
        let beat = t.samples_per_beat() as u64;
        let bar = beat * 4;
        let mut seq = Sequencer::new();
        seq.slot_mut(0).unwrap().set_clip(Some(four_on_the_floor()));
        // Ask mid-bar, as a human would.
        seq.launch(0, beat + 977, Quantize::Bar, &t);

        let hits = run(&mut seq, &t, bar * 2, 128);
        assert_eq!(hits.first().copied(), Some(bar));
    }

    #[test]
    fn two_pads_launched_apart_still_start_together() {
        let t = transport();
        let beat = t.samples_per_beat() as u64;
        let bar = beat * 4;
        let mut seq = Sequencer::new();
        seq.slot_mut(0).unwrap().set_clip(Some(four_on_the_floor()));
        seq.slot_mut(1).unwrap().set_clip(Some(four_on_the_floor()));
        seq.launch(0, 10, Quantize::Bar, &t);
        seq.launch(1, beat * 2 + 13, Quantize::Bar, &t);

        let mut out = [SeqEvent {
            frame: 0,
            slot: 0,
            kind: ClipEventKind::NoteOff {
                channel: 0,
                note: 0,
            },
        }; 64];
        let mut first: [Option<u64>; 2] = [None, None];
        let mut pos = 0u64;
        while pos < bar * 2 {
            let n = seq.collect(&t, pos, 128, &mut out);
            for ev in &out[..n] {
                if matches!(ev.kind, ClipEventKind::NoteOn { .. }) && first[ev.slot].is_none() {
                    first[ev.slot] = Some(pos + ev.frame as u64);
                }
            }
            pos += 128;
        }
        assert_eq!(first[0], Some(bar));
        assert_eq!(first[0], first[1], "same bar grid → same start frame");
    }

    #[test]
    fn stopping_flushes_held_notes() {
        let t = transport();
        let beat = t.samples_per_beat() as u64;
        let clip = Clip::new(
            vec![
                ClipEvent {
                    tick: 0,
                    kind: ClipEventKind::NoteOn {
                        channel: 0,
                        note: 60,
                        velocity: 100,
                    },
                },
                ClipEvent {
                    tick: PPQ * 8,
                    kind: ClipEventKind::NoteOff {
                        channel: 0,
                        note: 60,
                    },
                },
            ],
            PPQ * 8,
        );
        let mut seq = Sequencer::new();
        seq.slot_mut(0).unwrap().set_clip(Some(clip));
        seq.launch(0, 0, Quantize::Off, &t);

        let mut out = [SeqEvent {
            frame: 0,
            slot: 0,
            kind: ClipEventKind::NoteOff {
                channel: 0,
                note: 0,
            },
        }; 64];
        seq.collect(&t, 0, 128, &mut out);
        seq.stop(0, 200, Quantize::Off, &t);

        let mut offs = 0;
        let mut pos = 128u64;
        while pos < beat * 2 {
            let n = seq.collect(&t, pos, 128, &mut out);
            offs += out[..n]
                .iter()
                .filter(|e| matches!(e.kind, ClipEventKind::NoteOff { .. }))
                .count();
            pos += 128;
        }
        assert_eq!(offs, 1, "the held note must be released on stop");
        assert!(!seq.slot(0).unwrap().is_active());
    }

    #[test]
    fn tempo_change_rescales_the_grid() {
        let mut t = transport();
        let mut seq = Sequencer::new();
        seq.slot_mut(0).unwrap().set_clip(Some(four_on_the_floor()));
        seq.launch(0, 0, Quantize::Off, &t);

        let mut out = [SeqEvent {
            frame: 0,
            slot: 0,
            kind: ClipEventKind::NoteOff {
                channel: 0,
                note: 0,
            },
        }; 64];
        // Consume the downbeat, then double the tempo.
        seq.collect(&t, 0, 128, &mut out);
        t.set_bpm(240.0);
        let fast_beat = t.samples_per_beat() as u64; // 12_000

        let mut hits = Vec::new();
        let mut pos = 128u64;
        while pos < fast_beat * 4 {
            let n = seq.collect(&t, pos, 128, &mut out);
            for ev in &out[..n] {
                if matches!(ev.kind, ClipEventKind::NoteOn { .. }) {
                    hits.push(pos + ev.frame as u64);
                }
            }
            pos += 128;
        }
        assert_eq!(hits.first().copied(), Some(fast_beat));
    }

    #[test]
    fn empty_clip_never_launches() {
        let t = transport();
        let mut seq = Sequencer::new();
        seq.slot_mut(0)
            .unwrap()
            .set_clip(Some(Clip::new(vec![], 0)));
        seq.launch(0, 0, Quantize::Off, &t);
        assert!(!seq.slot(0).unwrap().is_active());
    }

    #[test]
    fn boxed_clip_swap_returns_the_previous_allocation() {
        let mut slot = ClipSlot::default();
        let first = Box::new(four_on_the_floor());
        let second = Box::new(four_on_the_floor());
        assert!(slot.swap_boxed(Some(first)).is_none());
        let previous = slot.swap_boxed(Some(second)).expect("first clip");
        assert!(!previous.is_empty());
        let cleared = slot.swap_boxed(None).expect("second clip");
        assert!(!cleared.is_empty());
        assert!(slot.clip().is_none());
    }
}
