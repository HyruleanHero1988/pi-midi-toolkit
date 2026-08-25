//! Instrument-surface state. Rendering and IPC consume this; they do not own notes.

use crate::client::Outbox;
use crate::kaoss_ui::{self, KaossPicker, KAOSS_PROGRAMS};
use crate::layout::{Hit, Layout, Rect, Surface};
use crate::mode::UiMode;
use crate::phrases::{self, PhrasePad};
use crate::presets::{self, PresetSnapshot};
use crate::seq::{SeqAction, SeqModel, SEQ_CLIP_SLOT};
use crate::songs::{self, SONG_CLIP_SLOT};
use crate::waves;
use jambox_protocol::{RepeatDivision, RepeatPhase, StatusReply, TouchPhase};
use std::path::PathBuf;

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
    pub pads_edit: bool,
    pub pads_clear_armed: bool,
    pub pads_selected: usize,
    pub status_line: String,
    pub synth_params: [f32; 5],
    pub wave_names: Vec<String>,
    pub morph_a: u16,
    pub morph_b: u16,
    /// `Some(true)` = picking A, `Some(false)` = picking B.
    pub synth_pick_a: Option<bool>,
    pub synth_pick_page: usize,
    pub kaoss_scale_index: u8,
    pub kaoss_key: u8,
    pub kaoss_octaves: u8,
    pub kaoss_full: bool,
    pub kaoss_hold: bool,
    pub kaoss_program: usize,
    pub kaoss_gate: usize,
    pub kaoss_picker: Option<KaossPicker>,
    kaoss_hold_gesture: Option<u32>,
    pub seq: SeqModel,
    pub preset_occupied: [bool; 8],
    pub preset_selected: usize,
    pub song_files: Vec<PathBuf>,
    pub song_selected: usize,
    pub song_scroll: usize,
    pub song_playing: bool,
    pub fx_bus: [f32; 3],
    pub log_lines: Vec<String>,
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
            pads_edit: false,
            pads_clear_armed: false,
            pads_selected: 0,
            status_line: String::new(),
            synth_params: [0.5, 0.5, 0.8, 0.05, 0.3],
            wave_names: waves::list_wave_names(&waves::waves_dirs_from_env()),
            morph_a: 0,
            morph_b: 1,
            synth_pick_a: None,
            synth_pick_page: 0,
            kaoss_scale_index: 1,
            kaoss_key: 0,
            kaoss_octaves: 2,
            kaoss_full: false,
            kaoss_hold: false,
            kaoss_program: 0,
            kaoss_gate: 0,
            kaoss_picker: None,
            kaoss_hold_gesture: None,
            seq: SeqModel::new(),
            preset_occupied: [false; 8],
            preset_selected: 0,
            song_files: Vec::new(),
            song_selected: 0,
            song_scroll: 0,
            song_playing: false,
            fx_bus: [0.0, 0.0, 0.0],
            log_lines: Vec::new(),
            fingers: [Finger::silent(); MAX_FINGERS],
            next_gesture: 1,
            cells: [[0; LED_COLS]; LED_ROWS],
            phrases_loaded: false,
        }
    }

    pub fn ensure_library_loaded(&mut self) {
        let presets_dir = presets::presets_dir_from_env();
        self.preset_occupied = presets::list_occupied(&presets_dir);
        let songs_dir = songs::songs_dir_from_env();
        self.song_files = songs::list_songs(&songs_dir);
        if self.wave_names.len() <= 4 {
            self.wave_names = waves::list_wave_names(&waves::waves_dirs_from_env());
        }
    }

    pub fn wave_label(&self, index: u16) -> &str {
        self.wave_names
            .get(index as usize)
            .map(|s| s.as_str())
            .unwrap_or("—")
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
        self.kaoss_picker = None;
        self.synth_pick_a = None;
        if mode != UiMode::Kaoss && self.kaoss_full {
            self.kaoss_full = false;
            self.layout.apply_kaoss_full(false);
        }
    }

    pub fn push_log(&mut self, line: impl Into<String>) {
        self.log_lines.push(line.into());
        if self.log_lines.len() > 14 {
            let drop = self.log_lines.len() - 14;
            self.log_lines.drain(0..drop);
        }
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

        let hit = if let Some(kind) = self.kaoss_picker {
            let base = self.layout.hit(self.mode, px, py);
            match base {
                Hit::KaossProg
                | Hit::KaossScale
                | Hit::KaossKey
                | Hit::KaossOct
                | Hit::KaossHold
                | Hit::KaossGate
                | Hit::KaossBpmUp
                | Hit::KaossBpmDown
                | Hit::KaossFull
                | Hit::Drum { .. }
                | Hit::Division(_)
                | Hit::Nav(_) => base,
                _ => self.layout.hit_kaoss_picker(kind, px, py),
            }
        } else if self.mode == UiMode::Synth && self.synth_pick_a.is_some() {
            let page_len = self.wave_names.len().saturating_sub(self.synth_pick_page * 12);
            self.layout
                .hit_synth_picker(px, py, self.synth_pick_page, page_len)
        } else {
            self.layout.hit(self.mode, px, py)
        };

        match hit {
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
                self.begin_kaoss_touch(gesture, x, y, outbox);
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
                if self.pads_edit && self.pads_clear_armed {
                    self.clear_phrase(index, outbox);
                } else if self.pads_edit {
                    self.pads_selected = index;
                    self.status_line = format!(
                        "{} · {}",
                        phrases::pad_label(index),
                        if self.phrases[index].empty {
                            "empty"
                        } else if self.phrases[index].loop_mode {
                            "LOOP"
                        } else {
                            "1SHOT"
                        }
                    );
                } else {
                    self.toggle_phrase(index, outbox);
                }
            }
            Hit::PadsPlayView => {
                self.tap_ui(slot, id, gesture, px, py);
                self.pads_edit = false;
                self.pads_clear_armed = false;
                self.layout.pads_clear.w = 0;
                self.layout.pads_trig.w = 0;
                self.status_line = "pads PLAY".into();
            }
            Hit::PadsEditView => {
                self.tap_ui(slot, id, gesture, px, py);
                self.pads_edit = true;
                self.layout.pads_clear = Rect {
                    x: 16,
                    y: crate::layout::HUD_H + (crate::layout::SCREEN_H
                        - crate::layout::HUD_H
                        - crate::layout::NAV_H)
                        - 56,
                    w: 200,
                    h: 48,
                };
                self.layout.pads_trig = Rect {
                    x: 224,
                    y: self.layout.pads_clear.y,
                    w: 200,
                    h: 48,
                };
                self.status_line = "pads EDIT — CLEAR then tap a pad, or TRIG for loop/1shot".into();
            }
            Hit::PadsClearArm => {
                self.tap_ui(slot, id, gesture, px, py);
                if !self.pads_edit {
                    self.status_line = "switch to EDIT first".into();
                } else {
                    self.pads_clear_armed = !self.pads_clear_armed;
                    self.status_line = if self.pads_clear_armed {
                        "CLEAR armed — tap a pad".into()
                    } else {
                        "CLEAR disarmed".into()
                    };
                }
            }
            Hit::PadsTrig => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_phrase_trig(outbox);
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
            Hit::SynthWaveA => {
                self.tap_ui(slot, id, gesture, px, py);
                self.open_synth_pick(true);
            }
            Hit::SynthWaveB => {
                self.tap_ui(slot, id, gesture, px, py);
                self.open_synth_pick(false);
            }
            Hit::SynthSwap => {
                self.tap_ui(slot, id, gesture, px, py);
                self.swap_morph_pair(outbox);
            }
            Hit::SynthPickDone => {
                self.tap_ui(slot, id, gesture, px, py);
                self.synth_pick_a = None;
                self.status_line.clear();
            }
            Hit::SynthPickPrev => {
                self.tap_ui(slot, id, gesture, px, py);
                if self.synth_pick_page > 0 {
                    self.synth_pick_page -= 1;
                }
            }
            Hit::SynthPickNext => {
                self.tap_ui(slot, id, gesture, px, py);
                let max_page = self.wave_names.len().saturating_sub(1) / 12;
                if self.synth_pick_page < max_page {
                    self.synth_pick_page += 1;
                }
            }
            Hit::SynthPickWave(index) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.assign_morph_endpoint(index, outbox);
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
            Hit::KaossProg => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_kaoss_picker(KaossPicker::Program);
            }
            Hit::KaossScale => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_kaoss_picker(KaossPicker::Scale);
            }
            Hit::KaossKey => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_kaoss_picker(KaossPicker::Key);
            }
            Hit::KaossOct => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_kaoss_picker(KaossPicker::Octave);
            }
            Hit::KaossHold => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_kaoss_hold(outbox);
            }
            Hit::KaossGate => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_kaoss_picker(KaossPicker::Gate);
            }
            Hit::KaossBpmUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_kaoss_bpm(5.0, outbox);
            }
            Hit::KaossBpmDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_kaoss_bpm(-5.0, outbox);
            }
            Hit::KaossFull => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_picker = None;
                self.kaoss_full = !self.kaoss_full;
                self.layout.apply_kaoss_full(self.kaoss_full);
                self.status_line = if self.kaoss_full {
                    "FULL PAD — tap EXIT to leave".into()
                } else {
                    "split pad".into()
                };
            }
            Hit::KaossPicker(index) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.apply_kaoss_picker(index, outbox);
            }
            Hit::KaossPickerClose => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_picker = None;
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
                let action = self.seq.toggle_record();
                self.apply_seq_action(action, outbox);
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
                let action = self.seq.toggle_play();
                self.apply_seq_action(action, outbox);
            }
            Hit::SeqKeep => {
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
                let action = self.seq.keep();
                self.apply_seq_action(action, outbox);
            }
            Hit::SeqDrop => {
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
                let action = self.seq.drop();
                self.apply_seq_action(action, outbox);
            }
            Hit::SeqUndo => {
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
                let action = self.seq.undo();
                self.apply_seq_action(action, outbox);
            }
            Hit::SeqLenDouble => {
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
                let action = self.seq.double_len();
                self.apply_seq_action(action, outbox);
            }
            Hit::SeqLenHalve => {
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
                let action = self.seq.halve_len();
                self.apply_seq_action(action, outbox);
            }
            Hit::SeqExtend => {
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
                self.seq.toggle_extend();
                self.status_line = self.seq.status.clone();
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
                let action = self.seq.stop_all();
                self.apply_seq_action(action, outbox);
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
                let action = self.seq.clear();
                self.apply_seq_action(action, outbox);
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
            Hit::PresetSlot(index) => {
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
                self.load_preset(index, outbox);
            }
            Hit::PresetSave => {
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
                self.save_preset(self.preset_selected);
            }
            Hit::SongRow(row) => {
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
                let idx = self.song_scroll + row;
                if idx < self.song_files.len() {
                    self.song_selected = idx;
                    self.status_line = self.song_files[idx]
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("song")
                        .to_string();
                }
            }
            Hit::SongPlay => {
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
                self.play_selected_song(outbox);
            }
            Hit::SongStop => {
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
                outbox.clip_stop(SONG_CLIP_SLOT, "off");
                self.song_playing = false;
                self.status_line = "song stop".into();
            }
            Hit::SongPrev => {
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
                if self.song_scroll > 0 {
                    self.song_scroll -= 1;
                }
            }
            Hit::SongNext => {
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
                if self.song_scroll + 5 < self.song_files.len() {
                    self.song_scroll += 1;
                }
            }
            Hit::SettingsPanic => {
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
                outbox.panic();
                self.status_line = "PANIC".into();
                self.push_log("panic");
            }
            Hit::SettingsAllOff => {
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
                outbox.all_notes_off();
                self.status_line = "all notes off".into();
                self.push_log("all notes off");
            }
            Hit::SettingsFx(index) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::SettingsFx { index },
                };
                self.apply_fx_slider(index, py, outbox);
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
                self.move_kaoss_touch(self.fingers[slot].gesture, x, y, outbox);
            }
            Surface::SynthSlider { index } => {
                self.apply_synth_slider(index, px, py, outbox);
            }
            Surface::SettingsFx { index } => {
                self.apply_fx_slider(index, py, outbox);
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
            Surface::Kaoss => self.end_kaoss_touch(finger.gesture, finger.x, finger.y, outbox),
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
                    outbox.note_off(DRUM_CHANNEL, note);
                    self.seq.push_note(false, DRUM_CHANNEL, note, 0);
                }
            }
            Surface::SynthKey { note } => {
                outbox.note_off(0, note);
                self.seq.push_note(false, 0, note, 0);
            }
            Surface::Phrase { .. }
            | Surface::SynthSlider { .. }
            | Surface::SettingsFx { .. }
            | Surface::UiTap => {}
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

    fn clear_phrase(&mut self, index: usize, outbox: &mut Outbox) {
        if index >= 16 {
            return;
        }
        self.pads_clear_armed = false;
        self.phrases[index] = PhrasePad::default();
        self.phrase_playing[index] = false;
        outbox.clip_clear(index as u8);
        outbox.clip_stop(index as u8, "off");
        self.pads_selected = index;
        self.status_line = format!("{} cleared", phrases::pad_label(index));
    }

    fn toggle_phrase_trig(&mut self, outbox: &mut Outbox) {
        if !self.pads_edit {
            self.status_line = "switch to EDIT first".into();
            return;
        }
        let index = self.pads_selected.min(15);
        if self.phrases[index].empty {
            self.status_line = format!("{} empty — nothing to TRIG", phrases::pad_label(index));
            return;
        }
        self.phrases[index].loop_mode = !self.phrases[index].loop_mode;
        let mode = if self.phrases[index].loop_mode {
            "loop"
        } else {
            "oneshot"
        };
        outbox.clip_load(
            index as u8,
            self.phrases[index].length_ticks,
            mode,
            self.phrases[index].events.clone(),
        );
        self.status_line = format!("{} → {}", phrases::pad_label(index), mode.to_ascii_uppercase());
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

    const FX_PARAM_NAMES: [&'static str; 3] = ["drive", "delay_mix", "reverb_mix"];

    fn apply_fx_slider(&mut self, index: usize, py: i32, outbox: &mut Outbox) {
        if index >= 3 {
            return;
        }
        let track = self.layout.settings_fx_slider(index);
        let y = 1.0 - ((py - track.y) as f32 / track.h.max(1) as f32);
        let value = y.clamp(0.0, 1.0);
        self.fx_bus[index] = value;
        outbox.fx_bus(Self::FX_PARAM_NAMES[index], value);
        self.status_line = format!("bus {} {:.2}", Self::FX_PARAM_NAMES[index], value);
    }

    fn tap_ui(&mut self, slot: usize, id: i32, gesture: u32, px: i32, py: i32) {
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

    fn toggle_kaoss_picker(&mut self, kind: KaossPicker) {
        self.kaoss_picker = match self.kaoss_picker {
            Some(open) if open == kind => None,
            _ => Some(kind),
        };
        if self.kaoss_picker.is_some() {
            self.status_line = match kind {
                KaossPicker::Program => "pick program".into(),
                KaossPicker::Scale => "pick scale".into(),
                KaossPicker::Key => "pick key".into(),
                KaossPicker::Octave => "pick range".into(),
                KaossPicker::Gate => "pick gate".into(),
            };
        }
    }

    fn apply_kaoss_picker(&mut self, index: usize, outbox: &mut Outbox) {
        let Some(kind) = self.kaoss_picker.take() else {
            return;
        };
        match kind {
            KaossPicker::Program => {
                self.kaoss_program = index % KAOSS_PROGRAMS.len();
                let p = kaoss_ui::program(self.kaoss_program);
                self.status_line = format!("program {}", p.label);
            }
            KaossPicker::Scale => {
                self.kaoss_scale_index = (index % jambox_core::KAOSS_SCALES.len()) as u8;
                self.push_kaoss_scale(outbox);
                self.status_line = jambox_core::kaoss_scale(self.kaoss_scale_index as usize)
                    .label
                    .to_string();
            }
            KaossPicker::Key => {
                self.kaoss_key = (index % 12) as u8;
                self.push_kaoss_scale(outbox);
                self.status_line =
                    format!("key {}", jambox_core::NOTE_NAMES[self.kaoss_key as usize]);
            }
            KaossPicker::Octave => {
                self.kaoss_octaves = ((index % 4) + 1) as u8;
                self.push_kaoss_scale(outbox);
                self.status_line = format!("{} oct", self.kaoss_octaves);
            }
            KaossPicker::Gate => {
                self.kaoss_gate = index % kaoss_ui::GATE_LABELS.len();
                self.status_line = kaoss_ui::GATE_LABELS[self.kaoss_gate].to_string();
            }
        }
    }

    fn push_kaoss_scale(&mut self, outbox: &mut Outbox) {
        let root = match self.kaoss_octaves {
            1 => 48,
            2 => 48,
            3 => 36,
            _ => 24,
        };
        outbox.kaoss_scale(
            self.kaoss_scale_index,
            self.kaoss_key,
            root,
            self.kaoss_octaves,
        );
    }

    fn nudge_kaoss_bpm(&mut self, delta: f32, outbox: &mut Outbox) {
        self.bpm = (self.bpm + delta).clamp(40.0, 240.0);
        self.seq.bpm = self.bpm;
        outbox.tempo(self.bpm);
        self.status_line = format!("tempo {:.0}", self.bpm);
    }

    fn toggle_kaoss_hold(&mut self, outbox: &mut Outbox) {
        self.kaoss_hold = !self.kaoss_hold;
        if !self.kaoss_hold {
            if let Some(gesture) = self.kaoss_hold_gesture.take() {
                outbox.touch(gesture, TouchPhase::Up, 0.0, 0.0);
            }
            self.status_line = "HOLD off".into();
        } else {
            self.status_line = "HOLD on — latch last pad".into();
        }
    }

    fn begin_kaoss_touch(&mut self, gesture: u32, x: f32, y: f32, outbox: &mut Outbox) {
        if let Some(held) = self.kaoss_hold_gesture.take() {
            outbox.touch(held, TouchPhase::Up, 0.0, 0.0);
        }
        let prog = kaoss_ui::program(self.kaoss_program);
        if prog.note {
            outbox.touch(gesture, TouchPhase::Down, x, y);
        }
        self.apply_kaoss_xy(prog, x, y, outbox);
    }

    fn move_kaoss_touch(&mut self, gesture: u32, x: f32, y: f32, outbox: &mut Outbox) {
        let prog = kaoss_ui::program(self.kaoss_program);
        if prog.note {
            outbox.touch(gesture, TouchPhase::Move, x, y);
        }
        self.apply_kaoss_xy(prog, x, y, outbox);
    }

    fn end_kaoss_touch(&mut self, gesture: u32, x: f32, y: f32, outbox: &mut Outbox) {
        let prog = kaoss_ui::program(self.kaoss_program);
        if self.kaoss_hold && prog.note {
            self.kaoss_hold_gesture = Some(gesture);
            self.status_line = "HOLD latched".into();
            return;
        }
        if prog.note {
            outbox.touch(gesture, TouchPhase::Up, x, y);
        }
    }

    fn apply_kaoss_xy(
        &mut self,
        prog: kaoss_ui::KaossProgram,
        x: f32,
        y: f32,
        outbox: &mut Outbox,
    ) {
        // Engine always maps Y→tone on note touches; for other Y params (and FX
        // programs) drive the synth/bus from the UI so MORPH/FILTER feel right.
        if prog.note {
            if prog.y_param != "tone" {
                outbox.synth(prog.y_param, y);
                if let Some(i) = Self::synth_param_index(prog.y_param) {
                    self.synth_params[i] = y;
                }
            }
        } else {
            if let Some(xp) = prog.x_param {
                if Self::is_bus_param(xp) {
                    outbox.fx_bus(xp, x);
                } else {
                    outbox.synth(xp, x);
                    if let Some(i) = Self::synth_param_index(xp) {
                        self.synth_params[i] = x;
                    }
                }
            }
            if Self::is_bus_param(prog.y_param) {
                outbox.fx_bus(prog.y_param, y);
            } else {
                outbox.synth(prog.y_param, y);
                if let Some(i) = Self::synth_param_index(prog.y_param) {
                    self.synth_params[i] = y;
                }
            }
        }
    }

    fn synth_param_index(name: &str) -> Option<usize> {
        match name {
            "morph" => Some(0),
            "tone" => Some(1),
            "level" => Some(2),
            "attack" => Some(3),
            "release" => Some(4),
            _ => None,
        }
    }

    fn is_bus_param(name: &str) -> bool {
        matches!(name, "drive" | "delay_mix" | "reverb_mix" | "delay_time")
    }

    fn apply_seq_action(&mut self, action: SeqAction, outbox: &mut Outbox) {
        match action {
            SeqAction::None => {}
            SeqAction::Stop => {
                outbox.clip_stop(SEQ_CLIP_SLOT, "off");
            }
            SeqAction::Clear => {
                outbox.clip_clear(SEQ_CLIP_SLOT);
                outbox.clip_stop(SEQ_CLIP_SLOT, "off");
            }
            SeqAction::Upload {
                events,
                length_ticks,
                launch,
            } => {
                outbox.tempo(self.seq.bpm);
                self.bpm = self.seq.bpm;
                outbox.clip_load(SEQ_CLIP_SLOT, length_ticks, "loop", events);
                if launch {
                    outbox.clip_launch(SEQ_CLIP_SLOT, "bar");
                }
            }
        }
        self.status_line = self.seq.status.clone();
    }

    fn open_synth_pick(&mut self, pick_a: bool) {
        self.synth_pick_a = Some(pick_a);
        self.synth_pick_page = 0;
        self.status_line = if pick_a {
            "pick wave A".into()
        } else {
            "pick wave B".into()
        };
    }

    fn swap_morph_pair(&mut self, outbox: &mut Outbox) {
        std::mem::swap(&mut self.morph_a, &mut self.morph_b);
        self.synth_params[0] = 1.0 - self.synth_params[0];
        outbox.morph_pair(self.morph_a, self.morph_b);
        outbox.synth("morph", self.synth_params[0]);
        self.status_line = format!(
            "swap {} ↔ {}",
            self.wave_label(self.morph_a),
            self.wave_label(self.morph_b)
        );
    }

    fn assign_morph_endpoint(&mut self, index: usize, outbox: &mut Outbox) {
        if index >= self.wave_names.len() {
            return;
        }
        let pick_a = self.synth_pick_a.unwrap_or(true);
        if pick_a {
            self.morph_a = index as u16;
        } else {
            self.morph_b = index as u16;
        }
        outbox.morph_pair(self.morph_a, self.morph_b);
        self.status_line = format!(
            "{} = {}",
            if pick_a { "A" } else { "B" },
            self.wave_label(index as u16)
        );
    }

    fn load_preset(&mut self, index: usize, outbox: &mut Outbox) {
        self.preset_selected = index.min(7);
        let dir = presets::presets_dir_from_env();
        let Some(p) = presets::load_slot(&dir, self.preset_selected) else {
            self.status_line = format!("slot {} empty — SAVE first", self.preset_selected + 1);
            return;
        };
        self.synth_params = [p.morph, p.tone, p.level, p.attack, p.release];
        self.morph_a = p.morph_a;
        self.morph_b = p.morph_b;
        outbox.morph_pair(p.morph_a, p.morph_b);
        outbox.synth("morph", p.morph);
        outbox.synth("tone", p.tone);
        outbox.synth("level", p.level);
        outbox.synth("attack", p.attack);
        outbox.synth("release", p.release);
        self.status_line = format!("loaded {}", p.name);
    }

    fn save_preset(&mut self, index: usize) {
        let dir = presets::presets_dir_from_env();
        let preset = PresetSnapshot {
            version: 1,
            name: format!("SLOT {}", index + 1),
            morph: self.synth_params[0],
            tone: self.synth_params[1],
            level: self.synth_params[2],
            attack: self.synth_params[3],
            release: self.synth_params[4],
            morph_a: self.morph_a,
            morph_b: self.morph_b,
        };
        if presets::save_slot(&dir, index.min(7), &preset) {
            self.preset_occupied[index.min(7)] = true;
            self.preset_selected = index.min(7);
            self.status_line = format!("saved {}", preset.name);
        } else {
            self.status_line = "preset save failed".into();
        }
    }

    fn play_selected_song(&mut self, outbox: &mut Outbox) {
        let Some(path) = self.song_files.get(self.song_selected).cloned() else {
            self.status_line = "no songs in songs/".into();
            return;
        };
        let Some((events, length_ticks, bpm)) = songs::load_smf_as_clip(&path) else {
            self.status_line = "could not parse SMF".into();
            return;
        };
        outbox.tempo(bpm);
        outbox.clip_load(SONG_CLIP_SLOT, length_ticks, "oneshot", events);
        outbox.clip_launch(SONG_CLIP_SLOT, "bar");
        self.song_playing = true;
        self.bpm = bpm;
        self.status_line = format!(
            "play {}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("song")
        );
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
