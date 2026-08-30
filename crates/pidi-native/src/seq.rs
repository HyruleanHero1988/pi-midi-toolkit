//! Overdub sequencer — behavioral parity with `apps/pidi/pidi/sequencer.py`.
//!
//! Backbone locks loop length; later takes become a pending layer (KEEP/DROP).
//! Flattened layers tile across the cycle. Playback is uploaded as one looping
//! clip so jambox-engine owns musical time.

use jambox_protocol::WireClipEvent;

use crate::phrases::{seconds_to_ticks, PPQ};

pub const SEQ_CLIP_SLOT: u8 = 15;
pub const MAX_CYCLES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqState {
    Empty,
    RecBackbone,
    Stopped,
    Playing,
    Overdub,
    Review,
}

#[derive(Debug, Clone)]
pub struct RecEvent {
    pub t: f64,
    pub on: bool,
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
}

#[derive(Debug, Clone, Default)]
struct SeqLayer {
    events: Vec<RecEvent>,
    span: usize,
    label: String,
}

impl SeqLayer {
    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
struct Sequence {
    cycle_len: f64,
    cycles: usize,
    layers: Vec<SeqLayer>,
    pending: Option<SeqLayer>,
}

impl Sequence {
    fn is_empty(&self) -> bool {
        self.cycle_len <= 0.0 || self.layers.is_empty()
    }

    fn total_len(&self) -> f64 {
        if self.cycle_len <= 0.0 {
            0.0
        } else {
            self.cycle_len * self.cycles.max(1) as f64
        }
    }

    fn max_span(&self) -> usize {
        let mut spans: Vec<usize> = self.layers.iter().map(|l| l.span.max(1)).collect();
        if let Some(p) = &self.pending {
            spans.push(p.span.max(1));
        }
        spans.into_iter().max().unwrap_or(1)
    }

    fn set_backbone(&mut self, events: Vec<RecEvent>, length: f64) {
        self.cycle_len = length.max(0.0);
        self.cycles = 1;
        self.layers = vec![SeqLayer {
            events,
            span: 1,
            label: "backbone".into(),
        }];
        self.pending = None;
    }

    fn keep_pending(&mut self) -> bool {
        let Some(mut layer) = self.pending.take() else {
            return false;
        };
        if layer.is_empty() {
            return false;
        }
        let span = layer.span.clamp(1, MAX_CYCLES);
        if span > self.cycles {
            self.cycles = span;
        }
        layer.span = span;
        layer.label = format!("layer {}", self.layers.len());
        self.layers.push(layer);
        true
    }

    fn drop_pending(&mut self) -> bool {
        let had = self
            .pending
            .as_ref()
            .map(|p| !p.is_empty())
            .unwrap_or(false);
        self.pending = None;
        had
    }

    fn undo_layer(&mut self) -> bool {
        if self.layers.len() <= 1 {
            return false;
        }
        self.layers.pop();
        self.cycles = self.cycles.min(self.max_span()).max(1);
        true
    }

    fn set_cycles(&mut self, want: usize) -> bool {
        let want = want.clamp(1, MAX_CYCLES);
        if self.cycle_len <= 0.0 || want < self.max_span() || want == self.cycles {
            return false;
        }
        self.cycles = want;
        true
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn flatten(&self, include_pending: bool) -> Vec<RecEvent> {
        if self.cycle_len <= 0.0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for layer in &self.layers {
            out.extend(tile_layer(layer, self.cycle_len, self.cycles));
        }
        if include_pending {
            if let Some(p) = &self.pending {
                out.extend(tile_layer(p, self.cycle_len, self.cycles));
            }
        }
        out.sort_by(|a, b| match a.t.partial_cmp(&b.t) {
            Some(ord) => ord.then_with(|| a.on.cmp(&b.on)),
            None => std::cmp::Ordering::Equal,
        });
        out
    }

    fn to_wire(&self, bpm: f32, include_pending: bool) -> (Vec<WireClipEvent>, u32) {
        let events = self.flatten(include_pending);
        let length_ticks = seconds_to_ticks(self.total_len(), bpm).max(PPQ);
        let wire = events
            .iter()
            .map(|e| WireClipEvent {
                tick: seconds_to_ticks(e.t, bpm),
                on: e.on,
                channel: e.channel,
                note: e.note,
                velocity: e.velocity,
            })
            .collect();
        (wire, length_ticks)
    }

    fn layer_line(&self) -> String {
        let mut parts = Vec::new();
        for (i, layer) in self.layers.iter().enumerate() {
            let name = if layer.label.is_empty() {
                format!("L{i}")
            } else {
                layer.label.clone()
            };
            parts.push(format!("{name}:{}ev×{}", layer.events.len(), layer.span));
        }
        if let Some(p) = &self.pending {
            if !p.events.is_empty() {
                parts.push(format!("[pending {}ev]", p.events.len()));
            }
        }
        if parts.is_empty() {
            "no layers".into()
        } else {
            parts.join(" · ")
        }
    }
}

fn tile_layer(layer: &SeqLayer, cycle_len: f64, cycles: usize) -> Vec<RecEvent> {
    if cycle_len <= 0.0 || cycles == 0 || layer.events.is_empty() {
        return Vec::new();
    }
    let span = layer.span.clamp(1, cycles);
    let repeats = (cycles / span).max(1);
    let period = span as f64 * cycle_len;
    let mut out = Vec::new();
    for rep in 0..repeats {
        let offset = rep as f64 * period;
        for ev in &layer.events {
            if ev.t >= period - 1e-9 && rep + 1 < repeats {
                continue;
            }
            out.push(RecEvent {
                t: ev.t + offset,
                on: ev.on,
                channel: ev.channel,
                note: ev.note,
                velocity: ev.velocity,
            });
        }
    }
    out
}

fn cycles_for_take(take_len: f64, cycle_len: f64) -> usize {
    if cycle_len <= 0.0 {
        return 1;
    }
    let needed = ((take_len - 1e-3) / cycle_len).ceil() as usize;
    needed.clamp(1, MAX_CYCLES)
}

#[derive(Debug)]
pub enum SeqAction {
    None,
    Upload {
        events: Vec<WireClipEvent>,
        length_ticks: u32,
        launch: bool,
    },
    Stop,
    Clear,
}

#[derive(Debug, Clone)]
pub struct SeqModel {
    pub state: SeqState,
    pub bpm: f32,
    pub status: String,
    pub layer_line: String,
    pub extend_mode: bool,
    sequence: Sequence,
    take: Vec<RecEvent>,
    take_started: Option<std::time::Instant>,
}

impl Default for SeqModel {
    fn default() -> Self {
        Self::new()
    }
}

impl SeqModel {
    pub fn new() -> Self {
        Self {
            state: SeqState::Empty,
            bpm: 120.0,
            status: "REC a backbone groove, then overdub layers".into(),
            layer_line: "no layers".into(),
            extend_mode: false,
            sequence: Sequence::default(),
            take: Vec::new(),
            take_started: None,
        }
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.state, SeqState::RecBackbone | SeqState::Overdub)
    }

    pub fn rec_origin(&self) -> Option<std::time::Instant> {
        self.take_started
    }

    #[cfg(test)]
    pub fn recorded_on_notes(&self) -> Vec<u8> {
        self.take.iter().filter(|e| e.on).map(|e| e.note).collect()
    }

    #[cfg(test)]
    pub fn recorded_on_times(&self) -> Vec<f64> {
        self.take.iter().filter(|e| e.on).map(|e| e.t).collect()
    }

    pub fn is_playing(&self) -> bool {
        matches!(
            self.state,
            SeqState::Playing | SeqState::Overdub | SeqState::Review
        )
    }

    pub fn has_pending(&self) -> bool {
        self.sequence
            .pending
            .as_ref()
            .map(|p| !p.is_empty())
            .unwrap_or(false)
            || self.state == SeqState::Overdub
    }

    pub fn layer_count(&self) -> usize {
        self.sequence.layers.len()
    }

    /// Flattened clip for SEQ → PAD / song export. Pending layers included.
    pub fn snapshot(&self) -> Option<(Vec<WireClipEvent>, u32, f64)> {
        if self.sequence.is_empty() {
            return None;
        }
        let (events, length_ticks) = self.sequence.to_wire(self.bpm, true);
        if events.is_empty() || length_ticks == 0 {
            return None;
        }
        Some((events, length_ticks, self.sequence.total_len()))
    }

    pub fn rec_label(&self) -> (&'static str, u32) {
        match self.state {
            SeqState::RecBackbone => ("STOP REC", 0xcc241d),
            SeqState::Overdub => ("STOP OVB", 0xcc241d),
            SeqState::Empty => ("REC BACKBONE", 0x9d0006),
            _ => ("REC OVERDUB", 0x9d0006),
        }
    }

    pub fn play_label(&self) -> (&'static str, u32) {
        if self.is_playing() && self.state != SeqState::Overdub {
            ("STOP", 0xd79921)
        } else if self.state == SeqState::Empty {
            ("PLAY", 0x3c3836)
        } else {
            ("PLAY", 0x689d6a)
        }
    }

    pub fn push_note(&mut self, on: bool, channel: u8, note: u8, velocity: u8) {
        let Some(started) = self.take_started else {
            return;
        };
        self.push_note_at(on, channel, note, velocity, started.elapsed().as_secs_f64());
    }

    pub fn push_note_at(&mut self, on: bool, channel: u8, note: u8, velocity: u8, t: f64) {
        if !self.is_recording() {
            return;
        }
        if self.take_started.is_none() {
            return;
        }
        self.take.push(RecEvent {
            t: t.max(0.0),
            on,
            channel: channel & 0x0f,
            note: note & 0x7f,
            velocity: if on { velocity.max(1).min(127) } else { 0 },
        });
        self.refresh_lines();
    }

    pub fn toggle_record(&mut self) -> SeqAction {
        match self.state {
            SeqState::RecBackbone => self.finish_backbone(),
            SeqState::Overdub => self.finish_overdub(),
            SeqState::Empty => self.start_backbone(),
            SeqState::Stopped | SeqState::Playing | SeqState::Review => self.start_overdub(),
        }
    }

    fn start_backbone(&mut self) -> SeqAction {
        self.take.clear();
        self.take_started = Some(std::time::Instant::now());
        self.state = SeqState::RecBackbone;
        self.refresh_lines();
        SeqAction::Stop
    }

    fn finish_backbone(&mut self) -> SeqAction {
        self.take_started = None;
        let (trimmed, length) = trim_take(&self.take);
        self.take.clear();
        if trimmed.is_empty() || length <= 0.0 {
            self.state = SeqState::Empty;
            self.status = "empty take — try again".into();
            self.refresh_lines();
            return SeqAction::None;
        }
        let trimmed = close_open_notes(trimmed, length);
        self.sequence.set_backbone(trimmed, length);
        self.state = SeqState::Playing;
        self.refresh_lines();
        let (events, length_ticks) = self.sequence.to_wire(self.bpm, false);
        SeqAction::Upload {
            events,
            length_ticks,
            launch: true,
        }
    }

    fn start_overdub(&mut self) -> SeqAction {
        if self.sequence.is_empty() {
            return self.start_backbone();
        }
        if self.sequence.pending.is_some() {
            let _ = self.sequence.keep_pending();
        }
        self.take.clear();
        self.take_started = Some(std::time::Instant::now());
        self.sequence.pending = Some(SeqLayer {
            events: Vec::new(),
            span: self.sequence.cycles.max(1),
            label: "pending".into(),
        });
        self.state = SeqState::Overdub;
        self.refresh_lines();
        let (events, length_ticks) = self.sequence.to_wire(self.bpm, false);
        SeqAction::Upload {
            events,
            length_ticks,
            launch: true,
        }
    }

    pub fn finish_overdub(&mut self) -> SeqAction {
        self.take_started = None;
        let raw = std::mem::take(&mut self.take);
        if raw.is_empty() {
            self.sequence.pending = None;
            self.state = SeqState::Playing;
            self.status = "empty overdub — dropped".into();
            self.refresh_lines();
            return SeqAction::None;
        }
        // Overdub is timed from take start ≈ loop restart (engine relaunches on
        // arm). Do NOT auto-trim — that would slam the first hit to t=0 and
        // destroy in-loop placement (Tk records phase, never trims overdubs).
        let span = if self.extend_mode {
            let take_len = raw.iter().map(|e| e.t).fold(0.0_f64, f64::max) + 1e-3;
            cycles_for_take(take_len, self.sequence.cycle_len)
        } else {
            1
        };
        let span_secs = if self.extend_mode {
            span as f64 * self.sequence.cycle_len
        } else {
            self.sequence.cycle_len.max(0.05)
        };
        let closed = close_open_notes(raw, span_secs);
        let events = if self.extend_mode {
            closed
        } else {
            let cycle = self.sequence.cycle_len.max(1e-6);
            closed
                .into_iter()
                .map(|mut e| {
                    e.t %= cycle;
                    e
                })
                .collect()
        };
        self.sequence.pending = Some(SeqLayer {
            events,
            span,
            label: "pending".into(),
        });
        self.state = SeqState::Review;
        self.refresh_lines();
        let (wire, length_ticks) = self.sequence.to_wire(self.bpm, true);
        SeqAction::Upload {
            events: wire,
            length_ticks,
            launch: true,
        }
    }

    pub fn keep(&mut self) -> SeqAction {
        if self.state == SeqState::Overdub {
            let _ = self.finish_overdub();
        }
        if !self.sequence.keep_pending() {
            self.status = "nothing to KEEP".into();
            return SeqAction::None;
        }
        self.state = SeqState::Playing;
        self.refresh_lines();
        let (events, length_ticks) = self.sequence.to_wire(self.bpm, false);
        SeqAction::Upload {
            events,
            length_ticks,
            launch: true,
        }
    }

    pub fn drop(&mut self) -> SeqAction {
        if self.state == SeqState::Overdub {
            self.take_started = None;
            self.take.clear();
        }
        let had = self.sequence.drop_pending();
        self.state = if self.sequence.is_empty() {
            SeqState::Empty
        } else {
            SeqState::Playing
        };
        self.status = if had {
            "overdub dropped".into()
        } else {
            "nothing to DROP".into()
        };
        self.refresh_lines();
        if self.sequence.is_empty() {
            SeqAction::Clear
        } else {
            let (events, length_ticks) = self.sequence.to_wire(self.bpm, false);
            SeqAction::Upload {
                events,
                length_ticks,
                launch: true,
            }
        }
    }

    pub fn undo(&mut self) -> SeqAction {
        if !self.sequence.undo_layer() {
            self.status = "can't UNDO backbone — CLEAR instead".into();
            return SeqAction::None;
        }
        self.state = SeqState::Playing;
        self.refresh_lines();
        let (events, length_ticks) = self.sequence.to_wire(self.bpm, false);
        SeqAction::Upload {
            events,
            length_ticks,
            launch: true,
        }
    }

    pub fn toggle_play(&mut self) -> SeqAction {
        if self.sequence.is_empty() {
            self.status = "nothing to play — REC a backbone first".into();
            return SeqAction::None;
        }
        if self.is_playing() && self.state != SeqState::Overdub {
            self.state = SeqState::Stopped;
            self.refresh_lines();
            return SeqAction::Stop;
        }
        if self.state == SeqState::Overdub {
            return SeqAction::None;
        }
        self.state = SeqState::Playing;
        self.refresh_lines();
        let (events, length_ticks) = self.sequence.to_wire(self.bpm, self.has_pending());
        SeqAction::Upload {
            events,
            length_ticks,
            launch: true,
        }
    }

    pub fn stop_all(&mut self) -> SeqAction {
        if self.is_recording() {
            self.take_started = None;
            self.take.clear();
        }
        self.state = if self.sequence.is_empty() {
            SeqState::Empty
        } else {
            SeqState::Stopped
        };
        self.refresh_lines();
        SeqAction::Stop
    }

    pub fn clear(&mut self) -> SeqAction {
        self.sequence.clear();
        self.take.clear();
        self.take_started = None;
        self.state = SeqState::Empty;
        self.refresh_lines();
        SeqAction::Clear
    }

    pub fn double_len(&mut self) -> SeqAction {
        if !self.sequence.set_cycles(self.sequence.cycles.saturating_mul(2)) {
            self.status = "can't LEN x2".into();
            return SeqAction::None;
        }
        self.refresh_lines();
        let (events, length_ticks) = self.sequence.to_wire(self.bpm, false);
        SeqAction::Upload {
            events,
            length_ticks,
            launch: self.is_playing(),
        }
    }

    pub fn halve_len(&mut self) -> SeqAction {
        if !self.sequence.set_cycles(self.sequence.cycles / 2) {
            self.status = "can't LEN /2".into();
            return SeqAction::None;
        }
        self.refresh_lines();
        let (events, length_ticks) = self.sequence.to_wire(self.bpm, false);
        SeqAction::Upload {
            events,
            length_ticks,
            launch: self.is_playing(),
        }
    }

    pub fn toggle_extend(&mut self) {
        self.extend_mode = !self.extend_mode;
        self.status = if self.extend_mode {
            "OVERDUB: EXTEND".into()
        } else {
            "OVERDUB: WRAP".into()
        };
    }

    pub fn nudge_bpm(&mut self, delta: f32) {
        self.bpm = (self.bpm + delta).clamp(40.0, 240.0);
        self.status = format!("tempo {:.0}", self.bpm);
    }

    fn refresh_lines(&mut self) {
        self.layer_line = self.sequence.layer_line();
        let span = format!(
            "{:.2}s x {} = {:.2}s",
            self.sequence.cycle_len,
            self.sequence.cycles,
            self.sequence.total_len()
        );
        self.status = match self.state {
            SeqState::Empty => "REC a backbone groove, then overdub layers".into(),
            SeqState::RecBackbone => "REC BACKBONE — play, then REC again to lock length".into(),
            SeqState::Overdub => {
                format!("OVERDUB over {span} — KEEP to flatten, DROP to abandon")
            }
            SeqState::Review => format!(
                "REVIEW — {} new · KEEP or DROP",
                self.sequence
                    .pending
                    .as_ref()
                    .map(|p| p.events.len())
                    .unwrap_or(0)
            ),
            SeqState::Playing => format!("playing {span}"),
            SeqState::Stopped => format!("stopped · {span}"),
        };
    }
}

fn trim_take(events: &[RecEvent]) -> (Vec<RecEvent>, f64) {
    trim_loop_take(events, 0.35, 0.05, 2.0)
}

/// Trim leading/trailing dead space from a free-timing take (Tk `trim_loop_take`).
///
/// - Shift so the first note-on starts at t=0.
/// - Trailing silence after the last note-on is capped to the largest inter-onset
///   gap (so a slow finger on STOP does not inflate the loop).
/// - Note-offs after the last hit are kept; trail is measured from ons.
pub fn trim_loop_take(
    events: &[RecEvent],
    default_gap: f64,
    min_gap: f64,
    max_gap: f64,
) -> (Vec<RecEvent>, f64) {
    if events.is_empty() {
        return (Vec::new(), 0.0);
    }
    let mut ons: Vec<f64> = events.iter().filter(|e| e.on).map(|e| e.t).collect();
    ons.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if ons.is_empty() {
        let t0 = events.iter().map(|e| e.t).fold(f64::INFINITY, f64::min);
        let shifted: Vec<RecEvent> = events
            .iter()
            .map(|e| RecEvent {
                t: (e.t - t0).max(0.0),
                on: e.on,
                channel: e.channel,
                note: e.note,
                velocity: e.velocity,
            })
            .collect();
        let length = shifted
            .iter()
            .map(|e| e.t)
            .fold(0.0_f64, f64::max)
            + min_gap;
        return (shifted, length.max(min_gap));
    }

    let t0 = ons[0];
    let gaps: Vec<f64> = ons.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 0.0).collect();
    let trail = if gaps.is_empty() {
        default_gap.clamp(min_gap, max_gap)
    } else {
        gaps.into_iter()
            .fold(f64::NEG_INFINITY, f64::max)
            .clamp(min_gap, max_gap)
    };

    let last_on = *ons.last().unwrap();
    let last_ev = events.iter().map(|e| e.t).fold(f64::NEG_INFINITY, f64::max);
    let end_abs = (last_on + trail).max(last_ev + 0.01);
    let length = (end_abs - t0).max(min_gap);

    let mut shifted: Vec<RecEvent> = events
        .iter()
        .filter(|e| e.t >= t0 - 1e-6)
        .map(|e| RecEvent {
            t: (e.t - t0).max(0.0),
            on: e.on,
            channel: e.channel,
            note: e.note,
            velocity: e.velocity,
        })
        .filter(|e| e.t <= length + 1e-6)
        .collect();
    if shifted.is_empty() {
        return (Vec::new(), 0.0);
    }
    shifted.sort_by(|a, b| {
        a.t.partial_cmp(&b.t)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| match (a.on, b.on) {
                (false, true) => std::cmp::Ordering::Less,
                (true, false) => std::cmp::Ordering::Greater,
                _ => a
                    .channel
                    .cmp(&b.channel)
                    .then(a.note.cmp(&b.note)),
            })
    });
    (shifted, length)
}

/// Give every note-on a note-off inside `span` (a take can end mid-note).
pub fn close_open_notes(events: Vec<RecEvent>, span: f64) -> Vec<RecEvent> {
    if span <= 0.0 {
        return events;
    }
    let mut held: std::collections::HashMap<(u8, u8), f64> = std::collections::HashMap::new();
    for ev in &events {
        let key = (ev.channel, ev.note);
        if ev.on {
            held.insert(key, ev.t);
        } else {
            held.remove(&key);
        }
    }
    if held.is_empty() {
        return events;
    }
    let end = (span - 1e-3).max(0.0);
    let mut out = events;
    for ((channel, note), on_t) in held {
        out.push(RecEvent {
            t: (on_t + 1e-3).max(end),
            on: false,
            channel,
            note,
            velocity: 0,
        });
    }
    out.sort_by(|a, b| {
        a.t.partial_cmp(&b.t)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| match (a.on, b.on) {
                (false, true) => std::cmp::Ordering::Less,
                (true, false) => std::cmp::Ordering::Greater,
                _ => a.channel.cmp(&b.channel).then(a.note.cmp(&b.note)),
            })
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backbone_then_overdub_keep() {
        let mut seq = SeqModel::new();
        assert!(matches!(seq.toggle_record(), SeqAction::Stop));
        seq.push_note(true, 9, 36, 100);
        std::thread::sleep(std::time::Duration::from_millis(40));
        seq.push_note(false, 9, 36, 0);
        assert!(matches!(seq.toggle_record(), SeqAction::Upload { .. }));
        assert_eq!(seq.state, SeqState::Playing);

        assert!(matches!(seq.toggle_record(), SeqAction::Upload { .. }));
        assert_eq!(seq.state, SeqState::Overdub);
        seq.push_note(true, 9, 37, 100);
        std::thread::sleep(std::time::Duration::from_millis(20));
        seq.push_note(false, 9, 37, 0);
        let _ = seq.finish_overdub();
        assert!(seq.has_pending() || seq.state == SeqState::Review);
        assert!(matches!(seq.keep(), SeqAction::Upload { .. }));
        assert_eq!(seq.layer_count(), 2);
    }

    #[test]
    fn undo_cannot_remove_backbone() {
        let mut seq = SeqModel::new();
        seq.sequence.set_backbone(
            vec![RecEvent {
                t: 0.0,
                on: true,
                channel: 9,
                note: 36,
                velocity: 100,
            }],
            1.0,
        );
        seq.state = SeqState::Stopped;
        assert!(matches!(seq.undo(), SeqAction::None));
    }

    #[test]
    fn trim_drops_leading_silence_and_caps_trail() {
        let events = vec![
            RecEvent {
                t: 0.40,
                on: true,
                channel: 9,
                note: 36,
                velocity: 100,
            },
            RecEvent {
                t: 0.50,
                on: false,
                channel: 9,
                note: 36,
                velocity: 0,
            },
            RecEvent {
                t: 0.75,
                on: true,
                channel: 9,
                note: 38,
                velocity: 100,
            },
            RecEvent {
                t: 0.85,
                on: false,
                channel: 9,
                note: 38,
                velocity: 0,
            },
        ];
        let (trimmed, length) = trim_loop_take(&events, 0.35, 0.05, 2.0);
        assert!((trimmed[0].t).abs() < 1e-6, "first on at 0");
        // Inter-onset gap is 0.35s → trail 0.35; length ≈ 0.35 + 0.35 = 0.70
        assert!(length < 1.0, "trail capped by inter-onset gap, got {length}");
        assert!(
            trimmed.iter().any(|e| e.note == 38 && e.on),
            "second hit kept"
        );
    }

    #[test]
    fn trim_keeps_hanging_note_off_past_trail() {
        let events = vec![
            RecEvent {
                t: 1.0,
                on: true,
                channel: 0,
                note: 60,
                velocity: 100,
            },
            RecEvent {
                t: 1.9,
                on: false,
                channel: 0,
                note: 60,
                velocity: 0,
            },
        ];
        let (trimmed, length) = trim_loop_take(&events, 0.35, 0.05, 2.0);
        assert!(
            length >= 0.89,
            "held note-off must extend past default trail, got {length}"
        );
        assert!(trimmed.iter().any(|e| !e.on && e.note == 60));
    }

    #[test]
    fn trim_single_hit_uses_default_gap() {
        let events = vec![
            RecEvent {
                t: 1.0,
                on: true,
                channel: 0,
                note: 60,
                velocity: 100,
            },
            RecEvent {
                t: 1.1,
                on: false,
                channel: 0,
                note: 60,
                velocity: 0,
            },
        ];
        let (trimmed, length) = trim_loop_take(&events, 0.35, 0.05, 2.0);
        assert!((trimmed[0].t).abs() < 1e-6);
        assert!((length - 0.35).abs() < 0.05, "single-hit trail ≈ default, got {length}");
    }

    #[test]
    fn close_open_notes_adds_missing_offs() {
        let events = vec![RecEvent {
            t: 0.0,
            on: true,
            channel: 0,
            note: 60,
            velocity: 100,
        }];
        let closed = close_open_notes(events, 1.0);
        assert_eq!(closed.len(), 2);
        assert!(!closed[1].on);
        assert!((closed[1].t - 0.999).abs() < 0.01);
    }
}
