//! Overdub SEQ — backbone record → loop clip on the engine.
//!
//! First take locks loop length (dead-air trimmed). Playback is a looping
//! `Clip` on a reserved engine slot so musical time stays in jambox-engine.

use jambox_protocol::WireClipEvent;

use crate::phrases::{seconds_to_ticks, PPQ};

pub const SEQ_CLIP_SLOT: u8 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqPhase {
    Idle,
    Recording,
    Playing,
}

#[derive(Debug, Clone)]
struct RecEvent {
    t: f64,
    on: bool,
    channel: u8,
    note: u8,
    velocity: u8,
}

#[derive(Debug, Clone)]
pub struct SeqModel {
    pub phase: SeqPhase,
    pub bpm: f32,
    pub status: String,
    events: Vec<RecEvent>,
    started: Option<std::time::Instant>,
    length_ticks: u32,
    has_clip: bool,
}

impl Default for SeqModel {
    fn default() -> Self {
        Self::new()
    }
}

impl SeqModel {
    pub fn new() -> Self {
        Self {
            phase: SeqPhase::Idle,
            bpm: 120.0,
            status: "REC locks the loop · PLAY runs it".into(),
            events: Vec::new(),
            started: None,
            length_ticks: 0,
            has_clip: false,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.phase == SeqPhase::Recording
    }

    pub fn toggle_record(&mut self) -> SeqRecAction {
        match self.phase {
            SeqPhase::Recording => self.finish_record(),
            SeqPhase::Playing | SeqPhase::Idle => {
                self.events.clear();
                self.started = Some(std::time::Instant::now());
                self.phase = SeqPhase::Recording;
                self.has_clip = false;
                self.length_ticks = 0;
                self.status = "recording backbone…".into();
                SeqRecAction::Started
            }
        }
    }

    pub fn push_note(&mut self, on: bool, channel: u8, note: u8, velocity: u8) {
        if self.phase != SeqPhase::Recording {
            return;
        }
        let Some(started) = self.started else {
            return;
        };
        self.events.push(RecEvent {
            t: started.elapsed().as_secs_f64(),
            on,
            channel: channel & 0x0f,
            note: note & 0x7f,
            velocity: if on { velocity.max(1).min(127) } else { 0 },
        });
    }

    fn finish_record(&mut self) -> SeqRecAction {
        self.phase = SeqPhase::Idle;
        self.started = None;
        let (trimmed, length_sec) = trim_take(&self.events);
        if trimmed.is_empty() || length_sec <= 0.0 {
            self.events.clear();
            self.status = "empty take".into();
            return SeqRecAction::Empty;
        }
        let wire: Vec<WireClipEvent> = trimmed
            .iter()
            .map(|e| WireClipEvent {
                tick: seconds_to_ticks(e.t, self.bpm),
                on: e.on,
                channel: e.channel,
                note: e.note,
                velocity: e.velocity,
            })
            .collect();
        self.length_ticks = seconds_to_ticks(length_sec, self.bpm).max(PPQ);
        self.has_clip = true;
        self.events.clear();
        self.status = format!("backbone {:.2}s · PLAY to loop", length_sec);
        SeqRecAction::Finished {
            events: wire,
            length_ticks: self.length_ticks,
        }
    }

    pub fn toggle_play(&mut self) -> SeqPlayAction {
        if !self.has_clip {
            self.status = "nothing to play — REC a take first".into();
            return SeqPlayAction::None;
        }
        match self.phase {
            SeqPhase::Playing => {
                self.phase = SeqPhase::Idle;
                self.status = "stopped".into();
                SeqPlayAction::Stop
            }
            SeqPhase::Idle | SeqPhase::Recording => {
                if self.phase == SeqPhase::Recording {
                    // ignore play while recording
                    return SeqPlayAction::None;
                }
                self.phase = SeqPhase::Playing;
                self.status = "playing loop".into();
                SeqPlayAction::Start
            }
        }
    }

    pub fn clear(&mut self) -> bool {
        let had = self.has_clip || self.phase != SeqPhase::Idle;
        self.phase = SeqPhase::Idle;
        self.events.clear();
        self.started = None;
        self.length_ticks = 0;
        self.has_clip = false;
        self.status = "cleared".into();
        had
    }

    pub fn nudge_bpm(&mut self, delta: f32) {
        self.bpm = (self.bpm + delta).clamp(40.0, 240.0);
        self.status = format!("tempo {:.0}", self.bpm);
    }
}

#[derive(Debug)]
pub enum SeqRecAction {
    Started,
    Empty,
    Finished {
        events: Vec<WireClipEvent>,
        length_ticks: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqPlayAction {
    None,
    Start,
    Stop,
}

fn trim_take(events: &[RecEvent]) -> (Vec<RecEvent>, f64) {
    if events.is_empty() {
        return (Vec::new(), 0.0);
    }
    let ons: Vec<f64> = events.iter().filter(|e| e.on).map(|e| e.t).collect();
    if ons.is_empty() {
        return (Vec::new(), 0.0);
    }
    let t0 = ons[0];
    let mut gaps = Vec::new();
    for w in ons.windows(2) {
        gaps.push(w[1] - w[0]);
    }
    let gap = gaps
        .into_iter()
        .fold(0.35_f64, f64::max)
        .clamp(0.05, 2.0);
    let last_on = *ons.last().unwrap();
    let length = (last_on - t0) + gap;
    let trimmed: Vec<RecEvent> = events
        .iter()
        .filter_map(|e| {
            let t = e.t - t0;
            if t < 0.0 || t > length {
                return None;
            }
            Some(RecEvent {
                t,
                on: e.on,
                channel: e.channel,
                note: e.note,
                velocity: e.velocity,
            })
        })
        .collect();
    (trimmed, length.max(0.05))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_shifts_first_hit_to_zero() {
        let events = vec![
            RecEvent {
                t: 1.0,
                on: true,
                channel: 9,
                note: 36,
                velocity: 100,
            },
            RecEvent {
                t: 1.2,
                on: false,
                channel: 9,
                note: 36,
                velocity: 0,
            },
            RecEvent {
                t: 1.5,
                on: true,
                channel: 9,
                note: 37,
                velocity: 100,
            },
        ];
        let (trimmed, len) = trim_take(&events);
        assert!((trimmed[0].t - 0.0).abs() < 1e-9);
        assert!(len > 0.5);
    }

    #[test]
    fn record_then_finish_builds_clip_events() {
        let mut seq = SeqModel::new();
        assert!(matches!(seq.toggle_record(), SeqRecAction::Started));
        seq.push_note(true, 9, 36, 100);
        std::thread::sleep(std::time::Duration::from_millis(50));
        seq.push_note(false, 9, 36, 0);
        match seq.toggle_record() {
            SeqRecAction::Finished { events, length_ticks } => {
                assert!(!events.is_empty());
                assert!(length_ticks > 0);
            }
            other => panic!("{other:?}"),
        }
    }
}
