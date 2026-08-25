//! Instrument-surface state. Rendering and IPC consume this; they do not own notes.

use crate::client::Outbox;
use crate::layout::{Hit, Layout, Surface};
use crate::mode::UiMode;
use crate::phrases::{self, PhrasePad};
use crate::seq::{SeqModel, SeqPhase, SeqPlayAction, SeqRecAction, SEQ_CLIP_SLOT};
use jambox_protocol::{RepeatDivision, RepeatPhase, StatusReply, TouchPhase};

pub const LED_COLS: usize = 12;
pub const LED_ROWS: usize = 7;
pub const KICK_NOTE: u8 = 36;
pub const DRUM_CHANNEL: u8 = 9;
pub const MAX_FINGERS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatDivisionChoice {
    Quarter,
    Eighth,
    EighthTriplet,
    Sixteenth,
}

impl RepeatDivisionChoice {
    pub fn from_index(index: usize) -> Self {
        match index % 4 {
            1 => Self::Eighth,
            2 => Self::EighthTriplet,
            3 => Self::Sixteenth,
            _ => Self::Quarter,
        }
    }

    pub fn as_wire(self) -> RepeatDivision {
        match self {
            Self::Quarter => RepeatDivision::Quarter,
            Self::Eighth => RepeatDivision::Eighth,
            Self::EighthTriplet => RepeatDivision::EighthTriplet,
            Self::Sixteenth => RepeatDivision::Sixteenth,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::EighthTriplet => "1/8T",
            Self::Sixteenth => "1/16",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Finger {
    active: bool,
    id: i32,
    gesture: u32,
    x: f32,
    y: f32,
    px: i32,
    py: i32,
    surface: Surface,
}

impl Finger {
    const fn silent() -> Self {
        Self {
            active: false,
            id: -1,
            gesture: 0,
            x: 0.0,
            y: 0.0,
            px: 0,
            py: 0,
            surface: Surface::UiTap,
        }
    }
}

pub struct NativeModel {
    pub layout: Layout,
    pub mode: UiMode,
    pub division: RepeatDivisionChoice,
    pub status: StatusReply,
    pub fps: f32,
    pub connected: bool,
    pub frame: u64,
    pub bpm: f32,
    pub phrases: [PhrasePad; 16],
    pub phrase_playing: [bool; 16],
    pub status_line: String,
    pub synth_params: [f32; 5],
    pub kaoss_scale_index: u8,
    pub kaoss_key: u8,
    pub kaoss_full: bool,
    pub seq: SeqModel,
    fingers: [Finger; MAX_FINGERS],
    next_gesture: u32,
    cells: [[u32; LED_COLS]; LED_ROWS],
    phrases_loaded: bool,
}

impl Default for NativeModel {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeModel {
    pub fn new() -> Self {
        Self {
            layout: Layout::new(),
            mode: UiMode::Kaoss,
            division: RepeatDivisionChoice::Quarter,
            status: StatusReply::default(),
            fps: 0.0,
            connected: false,
            frame: 0,
            bpm: 120.0,
            phrases: std::array::from_fn(|_| PhrasePad::default()),
            phrase_playing: [false; 16],
            status_line: String::new(),
            synth_params: [0.5, 0.5, 0.8, 0.05, 0.3],
            kaoss_scale_index: 1,
            kaoss_key: 0,
            kaoss_full: false,
            seq: SeqModel::new(),
            fingers: [Finger::silent(); MAX_FINGERS],
            next_gesture: 1,
            cells: [[0; LED_COLS]; LED_ROWS],
            phrases_loaded: false,
        }
    }

    pub fn ensure_phrases_loaded(&mut self, outbox: &mut Outbox) {
        if self.phrases_loaded {
            return;
        }
        self.phrases_loaded = true;
        let dir = phrases::phrases_dir_from_env();
        self.phrases = phrases::load_bank(&dir, self.bpm);
        let mut n = 0usize;
        for (slot, pad) in self.phrases.iter().enumerate() {
            if pad.empty {
                outbox.clip_clear(slot as u8);
                continue;
            }
            n += 1;
            outbox.clip_load(
                slot as u8,
                pad.length_ticks,
                if pad.loop_mode { "loop" } else { "oneshot" },
                pad.events.clone(),
            );
        }
        self.status_line = format!("phrases {}/16 from {}", n, dir.display());
    }

    pub fn set_mode(&mut self, mode: UiMode) {
        self.mode = mode;
        self.status_line.clear();
    }

    pub fn active_fingers(&self) -> usize {
        self.fingers.iter().filter(|f| f.active).count()
    }

    pub fn kaoss_finger(&self) -> Option<(f32, f32)> {
        self.fingers
            .iter()
            .find(|f| f.active && f.surface == Surface::Kaoss)
            .map(|f| (f.x, f.y))
    }

    pub fn cell(&self, col: usize, row: usize) -> u32 {
        self.cells[row][col]
    }

    pub fn tick(&mut self, dt: f32) {
        self.frame = self.frame.wrapping_add(1);
        if dt > 0.0001 {
            let inst = 1.0 / dt;
            self.fps = if self.fps <= 0.1 {
                inst
            } else {
                self.fps * 0.9 + inst * 0.1
            };
        }
        if self.mode == UiMode::Kaoss {
            self.paint_cells();
        }
    }

    pub fn finger_down(&mut self, id: i32, px: i32, py: i32, outbox: &mut Outbox) {
        if self.fingers.iter().any(|f| f.active && f.id == id) {
            self.finger_move(id, px, py, outbox);
            return;
        }
        let slot = match self.fingers.iter().position(|f| !f.active) {
            Some(s) => s,
            None => return,
        };
        let gesture = self.next_gesture;
        self.next_gesture = self.next_gesture.wrapping_add(1).max(1);

        match self.layout.hit(self.mode, px, py) {
            Hit::Nav(mode) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                self.set_mode(mode);
            }
            Hit::HomeTile(mode) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                self.set_mode(mode);
            }
            Hit::Kaoss { x, y } => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x,
                    y,
                    px,
                    py,
                    surface: Surface::Kaoss,
                };
                outbox.touch(gesture, TouchPhase::Down, x, y);
            }
            Hit::Drum { note, .. } => {
                let repeat = note == KICK_NOTE && self.mode == UiMode::Kaoss;
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::Drum { note, repeat },
                };
                if repeat {
                    outbox.repeat(
                        gesture,
                        RepeatPhase::Down,
                        note,
                        DRUM_CHANNEL,
                        110,
                        self.division.as_wire(),
                    );
                } else {
                    outbox.note_on(DRUM_CHANNEL, note, 110);
                    self.seq.push_note(true, DRUM_CHANNEL, note, 110);
                }
            }
            Hit::Division(index) => {
                self.division = RepeatDivisionChoice::from_index(index);
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
            }
            Hit::PhrasePad(index) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::Phrase { slot: index },
                };
                self.toggle_phrase(index, outbox);
            }
            Hit::StopAllClips => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                outbox.stop_all_clips();
                self.phrase_playing = [false; 16];
                self.status_line = "stop all clips".into();
            }
            Hit::SynthSlider(index) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::SynthSlider { index },
                };
                self.apply_synth_slider(index, px, py, outbox);
            }
            Hit::SynthKey(index) => {
                let note = 48 + index as u8;
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::SynthKey { note },
                };
                outbox.note_on(0, note, 110);
                self.seq.push_note(true, 0, note, 110);
            }
            Hit::KaossScale => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                self.cycle_kaoss_scale(1, outbox);
            }
            Hit::KaossKey => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                self.cycle_kaoss_key(1, outbox);
            }
            Hit::KaossFull => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                self.kaoss_full = !self.kaoss_full;
                self.layout.apply_kaoss_full(self.kaoss_full);
                self.status_line = if self.kaoss_full {
                    "full pad".into()
                } else {
                    "split pad".into()
                };
            }
            Hit::SeqRec => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                self.handle_seq_rec(outbox);
            }
            Hit::SeqPlay => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                self.handle_seq_play(outbox);
            }
            Hit::SeqStop => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                outbox.clip_stop(SEQ_CLIP_SLOT, "off");
                self.seq.phase = SeqPhase::Idle;
                self.seq.status = "stopped".into();
                self.status_line = self.seq.status.clone();
            }
            Hit::SeqClear => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                if self.seq.clear() {
                    outbox.clip_clear(SEQ_CLIP_SLOT);
                    outbox.clip_stop(SEQ_CLIP_SLOT, "off");
                }
                self.status_line = self.seq.status.clone();
            }
            Hit::SeqBpmUp => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                self.seq.nudge_bpm(1.0);
                outbox.tempo(self.seq.bpm);
                self.bpm = self.seq.bpm;
                self.status_line = self.seq.status.clone();
            }
            Hit::SeqBpmDown => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                };
                self.seq.nudge_bpm(-1.0);
                outbox.tempo(self.seq.bpm);
                self.bpm = self.seq.bpm;
                self.status_line = self.seq.status.clone();
            }
            Hit::None => {}
        }
    }

    pub fn finger_move(&mut self, id: i32, px: i32, py: i32, outbox: &mut Outbox) {
        let Some(slot) = self.fingers.iter().position(|f| f.active && f.id == id) else {
            return;
        };
        self.fingers[slot].px = px;
        self.fingers[slot].py = py;
        match self.fingers[slot].surface {
            Surface::Kaoss => {
                let (x, y) = self.layout.kaoss.pad_xy(px, py);
                self.fingers[slot].x = x;
                self.fingers[slot].y = y;
                outbox.touch(self.fingers[slot].gesture, TouchPhase::Move, x, y);
            }
            Surface::SynthSlider { index } => {
                self.apply_synth_slider(index, px, py, outbox);
            }
            _ => {}
        }
    }

    pub fn finger_up(&mut self, id: i32, outbox: &mut Outbox) {
        let Some(slot) = self.fingers.iter().position(|f| f.active && f.id == id) else {
            return;
        };
        let finger = self.fingers[slot];
        self.fingers[slot] = Finger::silent();
        match finger.surface {
            Surface::Kaoss => outbox.touch(finger.gesture, TouchPhase::Up, finger.x, finger.y),
            Surface::Drum { note, repeat } => {
                if repeat {
                    outbox.repeat(
                        finger.gesture,
                        RepeatPhase::Up,
                        note,
                        DRUM_CHANNEL,
                        110,
                        self.division.as_wire(),
                    );
                } else {
                    self.seq.push_note(false, DRUM_CHANNEL, note, 0);
                }
            }
            Surface::SynthKey { note } => {
                outbox.note_off(0, note);
                self.seq.push_note(false, 0, note, 0);
            }
            Surface::Phrase { .. } | Surface::SynthSlider { .. } | Surface::UiTap => {}
        }
    }

    pub fn cancel_all(&mut self, outbox: &mut Outbox) {
        let active: Vec<i32> = self
            .fingers
            .iter()
            .filter(|f| f.active)
            .map(|f| f.id)
            .collect();
        for id in active {
            self.finger_up(id, outbox);
        }
    }

    fn toggle_phrase(&mut self, index: usize, outbox: &mut Outbox) {
        if index >= 16 {
            return;
        }
        if self.phrases[index].empty {
            self.status_line = format!("{} empty", phrases::pad_label(index));
            return;
        }
        if self.phrase_playing[index] {
            outbox.clip_stop(index as u8, "bar");
            self.phrase_playing[index] = false;
            self.status_line = format!("{} stop", phrases::pad_label(index));
        } else {
            outbox.clip_launch(index as u8, "bar");
            self.phrase_playing[index] = true;
            self.status_line = format!("{} launch", phrases::pad_label(index));
        }
    }

    const SYNTH_PARAM_NAMES: [&'static str; 5] =
        ["morph", "tone", "level", "attack", "release"];

    fn apply_synth_slider(&mut self, index: usize, _px: i32, py: i32, outbox: &mut Outbox) {
        if index >= 5 {
            return;
        }
        let track = self.layout.synth_slider(index);
        let y = 1.0 - ((py - track.y) as f32 / track.h.max(1) as f32);
        let value = y.clamp(0.0, 1.0);
        self.synth_params[index] = value;
        outbox.synth(Self::SYNTH_PARAM_NAMES[index], value);
        self.status_line = format!("{} {:.2}", Self::SYNTH_PARAM_NAMES[index], value);
    }

    fn cycle_kaoss_scale(&mut self, step: i32, outbox: &mut Outbox) {
        let n = jambox_core::KAOSS_SCALES.len() as i32;
        let next = (self.kaoss_scale_index as i32 + step).rem_euclid(n) as u8;
        self.kaoss_scale_index = next;
        outbox.kaoss_scale(next, self.kaoss_key, 48, 2);
        let scale = jambox_core::kaoss_scale(next as usize);
        self.status_line = scale.label.to_string();
    }

    fn cycle_kaoss_key(&mut self, step: i32, outbox: &mut Outbox) {
        self.kaoss_key = ((self.kaoss_key as i32 + step).rem_euclid(12)) as u8;
        outbox.kaoss_scale(self.kaoss_scale_index, self.kaoss_key, 48, 2);
        self.status_line = format!("key {}", jambox_core::NOTE_NAMES[self.kaoss_key as usize]);
    }

    fn handle_seq_rec(&mut self, outbox: &mut Outbox) {
        match self.seq.toggle_record() {
            SeqRecAction::Started => {
                outbox.clip_stop(SEQ_CLIP_SLOT, "off");
            }
            SeqRecAction::Empty => {}
            SeqRecAction::Finished {
                events,
                length_ticks,
            } => {
                outbox.clip_load(SEQ_CLIP_SLOT, length_ticks, "loop", events);
            }
        }
        self.status_line = self.seq.status.clone();
    }

    fn handle_seq_play(&mut self, outbox: &mut Outbox) {
        match self.seq.toggle_play() {
            SeqPlayAction::Start => {
                outbox.tempo(self.seq.bpm);
                outbox.clip_launch(SEQ_CLIP_SLOT, "bar");
            }
            SeqPlayAction::Stop => {
                outbox.clip_stop(SEQ_CLIP_SLOT, "off");
            }
            SeqPlayAction::None => {}
        }
        self.status_line = self.seq.status.clone();
    }

    fn paint_cells(&mut self) {
        let t = self.frame as f32 / 60.0;
        let finger = self.kaoss_finger();
        for row in 0..LED_ROWS {
            for col in 0..LED_COLS {
                self.cells[row][col] = pad_led_rgb(col, row, t, finger);
            }
        }
    }
}

pub fn pad_led_rgb(col: usize, row: usize, t: f32, finger: Option<(f32, f32)>) -> u32 {
    let base = 0x18u32 + ((col + row) as u32 % 3) * 8;
    let mut r = base;
    let mut g = base + 8;
    let mut b = base + 24;
    if let Some((fx, fy)) = finger {
        let cx = (col as f32 + 0.5) / LED_COLS as f32;
        let cy = (row as f32 + 0.5) / LED_ROWS as f32;
        let dx = cx - fx;
        let dy = cy - fy;
        let d = (dx * dx + dy * dy).sqrt();
        let glow = (1.0 - d * 2.2).clamp(0.0, 1.0);
        let pulse = 0.65 + 0.35 * (t * 6.0).sin();
        r = (r as f32 + glow * 180.0 * pulse) as u32;
        g = (g as f32 + glow * 40.0) as u32;
        b = (b as f32 + glow * 120.0 * pulse) as u32;
    }
    ((r.min(255) << 16) | (g.min(255) << 8) | b.min(255)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Outbox;
    use jambox_protocol::Request;

    #[test]
    fn kaoss_gesture_emits_touch_edges() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let cell = model.layout.kaoss_cell(0, 0);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        for i in 0..8 {
            model.finger_move(1, cell.x + 4 + i, cell.y + 4, &mut out);
        }
        model.finger_up(1, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(r, Request::Touch { .. })));
    }

    #[test]
    fn a_snare_can_fire_while_kick_repeats() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let kick = model.layout.drum_cell(0);
        let snare = model.layout.drum_cell(1);
        model.finger_down(1, kick.x + 4, kick.y + 4, &mut out);
        model.finger_down(2, snare.x + 4, snare.y + 4, &mut out);
        assert_eq!(model.active_fingers(), 2);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Repeat {
                note: 36,
                phase: RepeatPhase::Down,
                ..
            }
        )));
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::NoteOn {
                note: 37,
                channel: 9,
                ..
            }
        )));
    }

    #[test]
    fn nav_changes_mode() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        assert_eq!(model.mode, UiMode::Kaoss);
        let cell = model.layout.nav_cell(UiMode::Pads.index());
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        assert_eq!(model.mode, UiMode::Pads);
    }

    #[test]
    fn synth_slider_emits_synth_command() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Synth);
        let mut out = Outbox::new();
        let track = model.layout.synth_slider(0);
        model.finger_down(1, track.x + 4, track.y + track.h / 2, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth {
                param,
                ..
            } if param == "morph"
        )));
    }

    #[test]
    fn five_contacts_are_tracked() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let k = model.layout.kaoss;
        for i in 0..5 {
            model.finger_down(i, k.x + 20 + i * 40, k.y + 40, &mut out);
        }
        assert_eq!(model.active_fingers(), 5);
        model.finger_down(99, k.x + 100, k.y + 100, &mut out);
        assert_eq!(model.active_fingers(), 5);
    }
}
