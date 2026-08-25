//! Instrument-surface state. Rendering and IPC consume this; they do not own notes.
//!
//! Remaining Tk gaps (see NATIVE_KIOSK.md): deeper Map remap UI.
//! Map/WIFI/UPDATE are appliance-oriented host hooks.

use crate::client::Outbox;
use crate::host;
use crate::kaoss_ui::{self, KaossPicker, KAOSS_PROGRAMS};
use crate::layout::{Hit, Layout, Rect, Surface, NAV_H, SCREEN_H};
use crate::mode::UiMode;
use crate::phrases::{self, PhrasePad};
use crate::presets::{self, PresetSnapshot};
use crate::seq::{SeqAction, SeqModel, SEQ_CLIP_SLOT};
use crate::session::{self, OutMode, SessionState};
use crate::songs::{self, SONG_CLIP_SLOT};
use crate::voice_bake;
use crate::waves;
use jambox_core::{kaoss_scale, note_at_x, scale_notes, velocity_at_y};
use jambox_protocol::{MidiNotice, RepeatDivision, RepeatPhase, StatusReply, TouchPhase, WireClipEvent};
use std::path::PathBuf;
use std::time::Instant;

pub const LED_COLS: usize = 12;
pub const LED_ROWS: usize = 7;
pub const KICK_NOTE: u8 = 36;
pub const DRUM_CHANNEL: u8 = 9;
pub const MAX_FINGERS: usize = 5;
/// Hold still on the bottom edge this long to leave FULL PAD (Tk uses ~700ms).
pub const KAOSS_PLAY_EXIT_MS: u64 = 500;
/// Bottom strip above the nav bar for full-pad exit.
pub const KAOSS_FULL_EXIT_EDGE_PX: i32 = 24;
const KAOSS_CC_X: u8 = 12;
const KAOSS_CC_Y: u8 = 13;
const KAOSS_CC_TOUCH: u8 = 92;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxEditTarget {
    Bus,
    Voice,
    DrumGroup,
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

#[derive(Debug, Clone)]
pub struct PadRecEvent {
    pub t: f64,
    pub on: bool,
    pub ch: u8,
    pub note: u8,
    pub vel: u8,
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
    pub pads_mode_armed: bool,
    pub pads_selected: usize,
    pub pads_recording: Option<usize>,
    pub pad_rec_events: Vec<PadRecEvent>,
    pub pad_rec_started: Option<Instant>,
    pub seq_to_pad_armed: bool,
    pub status_line: String,
    pub synth_params: [f32; 5],
    pub vibrato_always: f32,
    pub wave_names: Vec<String>,
    pub wave_bank: Option<jambox_core::WaveBank>,
    pub morph_a: u16,
    pub morph_b: u16,
    /// `Some(true)` = picking A, `Some(false)` = picking B.
    pub synth_pick_a: Option<bool>,
    pub synth_pick_page: usize,
    /// Kit macros: tone, noise/snap, pitch, decay.
    pub drum_macros: [f32; 4],
    pub kaoss_scale_index: u8,
    pub kaoss_key: u8,
    pub kaoss_octaves: u8,
    pub kaoss_full: bool,
    /// Finger parked in the bottom exit strip while full — hold to leave.
    pub kaoss_full_exit_since: Option<Instant>,
    pub kaoss_hold: bool,
    pub kaoss_program: usize,
    pub kaoss_gate: usize,
    pub kaoss_show_all: bool,
    pub kaoss_channel: u8,
    pub kaoss_gate_on: bool,
    pub kaoss_gate_t0: Option<Instant>,
    pub kaoss_latched_xy: (f32, f32),
    pub kaoss_touching: bool,
    pub kaoss_picker: Option<KaossPicker>,
    kaoss_hold_gesture: Option<u32>,
    kaoss_gate_gesture: Option<u32>,
    pub seq: SeqModel,
    pub preset_occupied: [bool; 8],
    pub preset_selected: usize,
    pub song_files: Vec<PathBuf>,
    pub song_selected: usize,
    pub song_scroll: usize,
    pub song_playing: bool,
    pub song_loop: bool,
    pub fx_bus: [f32; 3],
    pub fx_voice: [f32; 3],
    pub fx_drum: [f32; 3],
    pub fx_target: FxEditTarget,
    pub log_lines: Vec<String>,
    pub pads_out: OutMode,
    pub song_out: OutMode,
    pub kaoss_out: OutMode,
    /// Last USB Kaoss note (for retune / release).
    kaoss_usb_note: Option<u8>,
    kaoss_cc_x_sent: Option<u8>,
    kaoss_cc_y_sent: Option<u8>,
    kaoss_cc_touch_sent: Option<u8>,
    pub session_dirty: bool,
    pub last_autosave: Instant,
    /// false = CELLS (default), true = GLOW (larger soft finger blob).
    pub kaoss_viz_glow: bool,
    /// Trail points `(x, y, age)` with age 1 = fresh.
    kaoss_trail: Vec<(f32, f32, f32)>,
    /// Ripples `(x, y, age)` with age 0 = fresh → 1 = gone.
    kaoss_ripples: Vec<(f32, f32, f32)>,
    fingers: [Finger; MAX_FINGERS],
    next_gesture: u32,
    cells: [[u32; LED_COLS]; LED_ROWS],
    phrases_loaded: bool,
    session_loaded: bool,
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
            pads_mode_armed: false,
            pads_selected: 0,
            pads_recording: None,
            pad_rec_events: Vec::new(),
            pad_rec_started: None,
            seq_to_pad_armed: false,
            status_line: String::new(),
            synth_params: [0.5, 0.5, 0.8, 0.05, 0.3],
            vibrato_always: 0.0,
            wave_names: waves::list_wave_names(&waves::waves_dirs_from_env()),
            wave_bank: None,
            morph_a: 0,
            morph_b: 1,
            synth_pick_a: None,
            synth_pick_page: 0,
            drum_macros: [0.60, 0.45, 0.50, 0.55],
            kaoss_scale_index: 1,
            kaoss_key: 0,
            kaoss_octaves: 2,
            kaoss_full: false,
            kaoss_full_exit_since: None,
            kaoss_hold: false,
            kaoss_program: 0,
            kaoss_gate: 0,
            kaoss_show_all: false,
            kaoss_channel: 0,
            kaoss_gate_on: false,
            kaoss_gate_t0: None,
            kaoss_latched_xy: (0.5, 0.5),
            kaoss_touching: false,
            kaoss_picker: None,
            kaoss_hold_gesture: None,
            kaoss_gate_gesture: None,
            seq: SeqModel::new(),
            preset_occupied: [false; 8],
            preset_selected: 0,
            song_files: Vec::new(),
            song_selected: 0,
            song_scroll: 0,
            song_playing: false,
            song_loop: false,
            fx_bus: [0.0, 0.0, 0.0],
            fx_voice: [0.0, 0.0, 0.0],
            fx_drum: [0.0, 0.0, 0.0],
            fx_target: FxEditTarget::Bus,
            log_lines: Vec::new(),
            pads_out: OutMode::Both,
            song_out: OutMode::Both,
            kaoss_out: OutMode::Local,
            kaoss_usb_note: None,
            kaoss_cc_x_sent: None,
            kaoss_cc_y_sent: None,
            kaoss_cc_touch_sent: None,
            session_dirty: false,
            last_autosave: Instant::now(),
            kaoss_viz_glow: false,
            kaoss_trail: Vec::new(),
            kaoss_ripples: Vec::new(),
            fingers: [Finger::silent(); MAX_FINGERS],
            next_gesture: 1,
            cells: [[0; LED_COLS]; LED_ROWS],
            phrases_loaded: false,
            session_loaded: false,
        }
    }

    pub fn ensure_library_loaded(&mut self) {
        self.ensure_library_loaded_with(None);
    }

    /// Load presets/songs/wave bank, and once per process the factory `settings.json` session.
    pub fn ensure_library_loaded_with(&mut self, outbox: Option<&mut Outbox>) {
        let presets_dir = presets::presets_dir_from_env();
        self.preset_occupied = presets::list_occupied(&presets_dir);
        let songs_dir = songs::songs_dir_from_env();
        self.song_files = songs::list_songs(&songs_dir);
        if self.wave_names.len() <= 4 {
            self.wave_names = waves::list_wave_names(&waves::waves_dirs_from_env());
        }
        if self.wave_bank.is_none() {
            // Builtins-only scope bank — matches morph A/B indices within bank.len().
            self.wave_bank = Some(jambox_core::WaveBank::with_builtins());
            self.sync_wave_bank();
        }
        if !self.session_loaded {
            if let Some(outbox) = outbox {
                self.session_loaded = true;
                if let Some(s) = session::load(&session::session_path_from_env()) {
                    self.apply_session(&s, outbox);
                }
            }
        }
    }

    fn sync_wave_bank(&mut self) {
        let Some(bank) = self.wave_bank.as_mut() else {
            return;
        };
        let max = bank.len().saturating_sub(1);
        bank.set_morph_pair(
            (self.morph_a as usize).min(max),
            (self.morph_b as usize).min(max),
        );
        bank.set_morph(self.synth_params[0]);
        bank.rebuild_morph();
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
            self.kaoss_full_exit_since = None;
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

    pub fn tick(&mut self, dt: f32, outbox: &mut Outbox) {
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
            self.age_kaoss_viz(dt);
            self.paint_cells();
            self.tick_kaoss_full_exit();
        }
        self.tick_kaoss_gate(outbox);
    }

    pub fn mark_dirty(&mut self) {
        self.session_dirty = true;
    }

    pub fn apply_session(&mut self, s: &SessionState, outbox: &mut Outbox) {
        self.bpm = s.bpm.clamp(40.0, 240.0);
        self.seq.bpm = self.bpm;
        self.synth_params = [s.morph, s.tone, s.level, s.attack, s.release];
        self.vibrato_always = s.vibrato_always.clamp(0.0, 1.0);
        self.morph_a = s.morph_a;
        self.morph_b = s.morph_b;
        self.kaoss_scale_index = s.kaoss_scale_index;
        self.kaoss_key = s.kaoss_key.min(11);
        self.kaoss_octaves = s.kaoss_octaves.clamp(1, 4);
        self.kaoss_program = s.kaoss_program % KAOSS_PROGRAMS.len();
        self.kaoss_gate = s.kaoss_gate % kaoss_ui::GATE_PATTERNS.len();
        self.kaoss_hold = s.kaoss_hold;
        self.kaoss_show_all = s.kaoss_show_all;
        self.kaoss_channel = s.kaoss_channel & 0x0f;
        self.fx_bus = s.fx_bus;
        self.pads_out = s.pads_out;
        self.song_out = s.song_out;
        self.kaoss_out = s.kaoss_out;
        outbox.emit_mode("clips", self.pads_out.wire());
        outbox.emit_mode("kaoss", self.kaoss_out.wire());
        outbox.tempo(self.bpm);
        outbox.morph_pair(self.morph_a, self.morph_b);
        outbox.synth("morph", self.synth_params[0]);
        outbox.synth("tone", self.synth_params[1]);
        outbox.synth("level", self.synth_params[2]);
        outbox.synth("attack", self.synth_params[3]);
        outbox.synth("release", self.synth_params[4]);
        outbox.synth("vibrato_always", self.vibrato_always);
        for (i, name) in ["drive", "delay_mix", "reverb_mix"].iter().enumerate() {
            outbox.fx_bus(name, self.fx_bus[i]);
        }
        self.sync_wave_bank();
        self.push_kaoss_scale(outbox);
        self.session_dirty = false;
        self.status_line = "session loaded".into();
    }

    pub fn capture_session(&self) -> SessionState {
        SessionState {
            version: 1,
            bpm: self.bpm,
            morph: self.synth_params[0],
            tone: self.synth_params[1],
            level: self.synth_params[2],
            attack: self.synth_params[3],
            release: self.synth_params[4],
            morph_a: self.morph_a,
            morph_b: self.morph_b,
            kaoss_scale_index: self.kaoss_scale_index,
            kaoss_key: self.kaoss_key,
            kaoss_octaves: self.kaoss_octaves,
            kaoss_program: self.kaoss_program,
            kaoss_gate: self.kaoss_gate,
            kaoss_hold: self.kaoss_hold,
            fx_bus: self.fx_bus,
            kaoss_show_all: self.kaoss_show_all,
            kaoss_channel: self.kaoss_channel,
            vibrato_always: self.vibrato_always,
            mode: self.mode.label().to_ascii_lowercase(),
            pads_out: self.pads_out,
            song_out: self.song_out,
            kaoss_out: self.kaoss_out,
        }
    }

    pub fn maybe_autosave(&mut self) {
        if !self.session_dirty {
            return;
        }
        if self.last_autosave.elapsed().as_secs_f32() < 5.0 {
            return;
        }
        let path = session::session_path_from_env();
        if session::save(&path, &self.capture_session()) {
            self.session_dirty = false;
            self.last_autosave = Instant::now();
        }
    }

    pub fn on_midi_notice(&mut self, notice: &MidiNotice) {
        let kind = notice.kind.to_ascii_lowercase();
        let note = notice.note.unwrap_or(0);
        let vel = notice.velocity.unwrap_or(0) as u8;
        if kind == "note_on" || kind == "noteon" {
            if vel > 0 {
                self.seq.push_note(true, notice.channel, note, vel);
                self.push_pad_rec(true, notice.channel, note, vel);
                self.push_log(format!("midi on ch{} n{} v{}", notice.channel, note, vel));
            } else {
                self.seq.push_note(false, notice.channel, note, 0);
                self.push_pad_rec(false, notice.channel, note, 0);
                self.push_log(format!("midi off ch{} n{}", notice.channel, note));
            }
        } else if kind == "note_off" || kind == "noteoff" {
            self.seq.push_note(false, notice.channel, note, 0);
            self.push_pad_rec(false, notice.channel, note, 0);
            self.push_log(format!("midi off ch{} n{}", notice.channel, note));
        }
    }

    pub fn panic_ui_state(&mut self, outbox: &mut Outbox) {
        self.phrase_playing = [false; 16];
        self.song_playing = false;
        self.kaoss_hold = false;
        self.kaoss_touching = false;
        self.kaoss_gate_on = false;
        self.kaoss_hold_gesture = None;
        self.kaoss_gate_gesture = None;
        let action = self.seq.stop_all();
        self.apply_seq_action(action, outbox);
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
                | Hit::KaossShowAll
                | Hit::KaossChannel
                | Hit::KaossSettings
                | Hit::KaossWipeFx
                | Hit::KaossViz
                | Hit::KaossOut
                | Hit::Drum { .. }
                | Hit::Division(_)
                | Hit::Nav(_) => base,
                _ => self
                    .layout
                    .hit_kaoss_picker(kind, px, py, self.kaoss_show_all),
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
                self.watch_kaoss_full_exit(py, true);
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
                    self.push_pad_rec(true, DRUM_CHANNEL, note, 110);
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
                if self.seq_to_pad_armed {
                    self.finish_seq_to_pad(index, outbox);
                } else if self.pads_edit && self.pads_clear_armed {
                    self.clear_phrase(index, outbox);
                } else if self.pads_edit && self.pads_mode_armed {
                    self.pads_selected = index;
                    self.pads_mode_armed = false;
                    self.toggle_phrase_trig(outbox);
                } else if self.pads_edit && self.pads_recording.is_some() {
                    let slot = self.pads_recording.unwrap_or(index);
                    self.status_line = format!("{} recording…", phrases::pad_label(slot));
                } else if self.pads_edit {
                    self.pads_selected = index;
                    self.status_line = format!(
                        "{} · {} · gain {:.1}",
                        phrases::pad_label(index),
                        if self.phrases[index].empty {
                            "empty"
                        } else if self.phrases[index].loop_mode {
                            "LOOP"
                        } else {
                            "1SHOT"
                        },
                        self.phrases[index].gain
                    );
                } else {
                    self.toggle_phrase(index, outbox);
                }
            }
            Hit::PadsPlayView => {
                self.tap_ui(slot, id, gesture, px, py);
                self.pads_edit = false;
                self.pads_clear_armed = false;
                self.pads_mode_armed = false;
                self.hide_pads_edit_chrome();
                self.status_line = "pads PLAY".into();
            }
            Hit::PadsEditView => {
                self.tap_ui(slot, id, gesture, px, py);
                self.pads_edit = true;
                self.show_pads_edit_chrome();
                self.status_line =
                    "pads EDIT — CLEAR/MODE/REC/VOL, or TRIG for loop/1shot".into();
            }
            Hit::PadsClearArm => {
                self.tap_ui(slot, id, gesture, px, py);
                if !self.pads_edit {
                    self.status_line = "switch to EDIT first".into();
                } else {
                    self.pads_clear_armed = !self.pads_clear_armed;
                    self.pads_mode_armed = false;
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
            Hit::PadsModeArm => {
                self.tap_ui(slot, id, gesture, px, py);
                if !self.pads_edit {
                    self.status_line = "switch to EDIT first".into();
                } else {
                    self.pads_mode_armed = !self.pads_mode_armed;
                    self.pads_clear_armed = false;
                    self.status_line = if self.pads_mode_armed {
                        "MODE armed — tap pad to toggle LOOP/1SHOT".into()
                    } else {
                        "MODE disarmed".into()
                    };
                }
            }
            Hit::PadsRec => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_pad_record(outbox);
            }
            Hit::PadsVolUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_pad_gain(phrases::PHRASE_GAIN_STEP, outbox);
            }
            Hit::PadsVolDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_pad_gain(-phrases::PHRASE_GAIN_STEP, outbox);
            }
            Hit::PadsVoice => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_pads_voice(outbox);
            }
            Hit::PadsChannel => {
                self.tap_ui(slot, id, gesture, px, py);
                self.cycle_pads_channel();
            }
            Hit::PadsSynth => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_pads_local_synth();
            }
            Hit::PadsOut => {
                self.tap_ui(slot, id, gesture, px, py);
                self.pads_out = self.pads_out.cycle();
                outbox.emit_mode("clips", self.pads_out.wire());
                self.status_line = self.pads_out.label().into();
                self.push_log(format!("pads {}", self.pads_out.label()));
                self.mark_dirty();
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
            Hit::SynthSaveAs => {
                self.tap_ui(slot, id, gesture, px, py);
                self.save_voice_as(outbox);
            }
            Hit::SynthVibUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_vibrato(0.05, outbox);
            }
            Hit::SynthVibDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_vibrato(-0.05, outbox);
            }
            Hit::DrumMacro(index) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_drum_macro(index, outbox);
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
                self.push_pad_rec(true, 0, note, 110);
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
                if self.kaoss_full {
                    self.leave_kaoss_full();
                } else {
                    self.kaoss_full = true;
                    self.kaoss_full_exit_since = None;
                    self.layout.apply_kaoss_full(true);
                    self.status_line = "hold bottom edge to exit".into();
                    self.mark_dirty();
                }
            }
            Hit::KaossShowAll | Hit::KaossSettings => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_show_all = !self.kaoss_show_all;
                self.status_line = if self.kaoss_show_all {
                    "SHOW ALL programs".into()
                } else {
                    "starter programs".into()
                };
                self.mark_dirty();
            }
            Hit::KaossViz => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_viz_glow = !self.kaoss_viz_glow;
                self.status_line = if self.kaoss_viz_glow {
                    "PAD VIZ: GLOW".into()
                } else {
                    "PAD VIZ: CELLS".into()
                };
            }
            Hit::KaossWipeFx => {
                self.tap_ui(slot, id, gesture, px, py);
                self.wipe_kaoss_fx(outbox);
            }
            Hit::KaossChannel => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_channel = (self.kaoss_channel + 1) & 0x0f;
                self.status_line = format!("kaoss CH {}", self.kaoss_channel + 1);
                self.mark_dirty();
            }
            Hit::KaossOut => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_out = self.kaoss_out.cycle();
                outbox.emit_mode("kaoss", self.kaoss_out.wire());
                self.status_line = self.kaoss_out.label().into();
                self.push_log(format!("kaoss {}", self.kaoss_out.label()));
                self.mark_dirty();
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
                self.mark_dirty();
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
                self.mark_dirty();
            }
            Hit::SeqToPad => {
                self.tap_ui(slot, id, gesture, px, py);
                self.arm_seq_to_pad();
            }
            Hit::SeqAllOff => {
                self.tap_ui(slot, id, gesture, px, py);
                outbox.all_notes_off();
                outbox.stop_all_clips();
                self.phrase_playing = [false; 16];
                self.status_line = "all off".into();
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
            Hit::PresetLoad => {
                self.tap_ui(slot, id, gesture, px, py);
                self.load_preset(self.preset_selected, outbox);
            }
            Hit::PresetDelete => {
                self.tap_ui(slot, id, gesture, px, py);
                self.delete_preset(self.preset_selected);
            }
            Hit::PresetFactory => {
                self.tap_ui(slot, id, gesture, px, py);
                self.factory_reset_synth(outbox);
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
            Hit::SongLoop => {
                self.tap_ui(slot, id, gesture, px, py);
                self.song_loop = !self.song_loop;
                self.status_line = if self.song_loop {
                    "song LOOP".into()
                } else {
                    "song ONESHOT".into()
                };
            }
            Hit::SongDelete => {
                self.tap_ui(slot, id, gesture, px, py);
                self.delete_selected_song();
            }
            Hit::SongBpmUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_kaoss_bpm(1.0, outbox);
            }
            Hit::SongBpmDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_kaoss_bpm(-1.0, outbox);
            }
            Hit::SongSaveSeq => {
                self.tap_ui(slot, id, gesture, px, py);
                self.save_seq_as_song();
            }
            Hit::SongOut => {
                self.tap_ui(slot, id, gesture, px, py);
                self.song_out = self.song_out.cycle();
                outbox.emit_mode("clips", self.song_out.wire());
                self.status_line = self.song_out.label().into();
                self.push_log(format!("song {}", self.song_out.label()));
                self.mark_dirty();
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
                self.panic_ui_state(outbox);
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
            Hit::SettingsFxTarget => {
                self.tap_ui(slot, id, gesture, px, py);
                self.cycle_fx_target();
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
            Hit::SettingsWifi => {
                self.tap_ui(slot, id, gesture, px, py);
                let (status, lines) = host::wifi_action();
                self.status_line = status;
                for line in lines {
                    self.push_log(line);
                }
            }
            Hit::SettingsUpdate => {
                self.tap_ui(slot, id, gesture, px, py);
                let (status, lines) = host::update_check();
                self.status_line = status;
                for line in lines {
                    self.push_log(line);
                }
            }
            Hit::MapThruOn => {
                self.tap_ui(slot, id, gesture, px, py);
                let (status, lines) = host::map_thru_on();
                self.status_line = status;
                for line in lines {
                    self.push_log(line);
                }
            }
            Hit::MapThruOff => {
                self.tap_ui(slot, id, gesture, px, py);
                let (status, lines) = host::map_thru_off();
                self.status_line = status;
                for line in lines {
                    self.push_log(line);
                }
            }
            Hit::MapRefresh => {
                self.tap_ui(slot, id, gesture, px, py);
                let (status, lines) = host::map_list_ports();
                self.status_line = status;
                for line in lines {
                    self.push_log(line);
                }
            }
            Hit::LogClear => {
                self.tap_ui(slot, id, gesture, px, py);
                self.log_lines.clear();
                self.status_line = "log cleared".into();
            }
            Hit::LogAllOff => {
                self.tap_ui(slot, id, gesture, px, py);
                outbox.all_notes_off();
                outbox.stop_all_clips();
                self.phrase_playing = [false; 16];
                self.status_line = "all notes off".into();
                self.push_log("all notes off");
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
                self.watch_kaoss_full_exit(py, true);
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
            Surface::Kaoss => {
                self.watch_kaoss_full_exit(0, false);
                self.end_kaoss_touch(finger.gesture, finger.x, finger.y, outbox);
            }
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
                    self.push_pad_rec(false, DRUM_CHANNEL, note, 0);
                }
            }
            Surface::SynthKey { note } => {
                outbox.note_off(0, note);
                self.seq.push_note(false, 0, note, 0);
                self.push_pad_rec(false, 0, note, 0);
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
            let pad = &self.phrases[index];
            if pad.voice_locked {
                outbox.morph_pair(pad.morph_a, pad.morph_b);
                outbox.synth("morph", pad.morph);
            }
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
        let dir = phrases::phrases_dir_from_env();
        let _ = phrases::delete_pad(&dir, index);
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
        let dir = phrases::phrases_dir_from_env();
        let _ = phrases::save_pad(&dir, index, &self.phrases[index], self.bpm);
        self.status_line = format!("{} → {}", phrases::pad_label(index), mode.to_ascii_uppercase());
    }

    fn show_pads_edit_chrome(&mut self) {
        let y = crate::layout::HUD_H
            + (crate::layout::SCREEN_H - crate::layout::HUD_H - crate::layout::NAV_H)
            - 56;
        self.layout.pads_clear = Rect {
            x: 8,
            y,
            w: 64,
            h: 48,
        };
        self.layout.pads_trig = Rect {
            x: 76,
            y,
            w: 80,
            h: 48,
        };
        self.layout.pads_mode = Rect {
            x: 160,
            y,
            w: 64,
            h: 48,
        };
        self.layout.pads_voice = Rect {
            x: 228,
            y,
            w: 72,
            h: 48,
        };
        self.layout.pads_synth = Rect {
            x: 304,
            y,
            w: 72,
            h: 48,
        };
        self.layout.pads_channel = Rect {
            x: 380,
            y,
            w: 64,
            h: 48,
        };
        self.layout.pads_rec = Rect {
            x: 448,
            y,
            w: 56,
            h: 48,
        };
        self.layout.pads_vol_down = Rect {
            x: 508,
            y,
            w: 48,
            h: 48,
        };
        self.layout.pads_vol_up = Rect {
            x: 560,
            y,
            w: 48,
            h: 48,
        };
        // OUT lives on the play footer; hide while edit chrome uses that strip.
        self.layout.pads_out.w = 0;
    }

    fn hide_pads_edit_chrome(&mut self) {
        self.layout.pads_clear.w = 0;
        self.layout.pads_trig.w = 0;
        self.layout.pads_mode.w = 0;
        self.layout.pads_voice.w = 0;
        self.layout.pads_synth.w = 0;
        self.layout.pads_channel.w = 0;
        self.layout.pads_rec.w = 0;
        self.layout.pads_vol_down.w = 0;
        self.layout.pads_vol_up.w = 0;
        let y = crate::layout::HUD_H
            + (crate::layout::SCREEN_H - crate::layout::HUD_H - crate::layout::NAV_H)
            - 56;
        self.layout.pads_out = Rect {
            x: 540,
            y,
            w: 120,
            h: 48,
        };
    }

    fn arm_seq_to_pad(&mut self) {
        if self.seq.snapshot().is_none() {
            self.status_line = "SEQ empty — nothing to assign".into();
            self.seq_to_pad_armed = false;
            return;
        }
        self.seq_to_pad_armed = true;
        self.pads_edit = true;
        self.set_mode(UiMode::Pads);
        self.show_pads_edit_chrome();
        self.status_line = "→PAD armed — tap a pad slot".into();
    }

    fn finish_seq_to_pad(&mut self, index: usize, outbox: &mut Outbox) {
        self.seq_to_pad_armed = false;
        let Some((events, length_ticks, _)) = self.seq.snapshot() else {
            self.status_line = "SEQ empty".into();
            return;
        };
        let pad = phrases::from_wire(events, length_ticks, self.seq.bpm, true);
        self.phrases[index] = pad;
        let dir = phrases::phrases_dir_from_env();
        let _ = phrases::save_pad(&dir, index, &self.phrases[index], self.bpm);
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
        self.pads_selected = index;
        self.pads_edit = false;
        self.hide_pads_edit_chrome();
        self.status_line = format!("{} ← SEQ", phrases::pad_label(index));
        self.push_log(format!("seq→pad {}", phrases::pad_label(index)));
    }

    fn toggle_pad_record(&mut self, outbox: &mut Outbox) {
        if !self.pads_edit {
            self.status_line = "switch to EDIT first".into();
            return;
        }
        if let Some(slot) = self.pads_recording.take() {
            self.finish_pad_record(slot, outbox);
            return;
        }
        let index = self.pads_selected.min(15);
        self.pads_recording = Some(index);
        self.pad_rec_events.clear();
        self.pad_rec_started = Some(Instant::now());
        self.status_line = format!(
            "REC {} — play notes, STOP REC to keep",
            phrases::pad_label(index)
        );
    }

    fn finish_pad_record(&mut self, index: usize, outbox: &mut Outbox) {
        let started = self.pad_rec_started.take();
        let events = std::mem::take(&mut self.pad_rec_events);
        if events.is_empty() {
            self.status_line = format!("{} REC empty — dropped", phrases::pad_label(index));
            return;
        }
        let length_secs = started
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.05)
            .max(0.05);
        let last_t = events.iter().map(|e| e.t).fold(0.0_f64, f64::max);
        let length_secs = last_t.max(0.05).min(length_secs) + 0.05;
        let length_ticks = phrases::seconds_to_ticks(length_secs, self.bpm);
        let wire: Vec<WireClipEvent> = events
            .iter()
            .map(|e| WireClipEvent {
                tick: phrases::seconds_to_ticks(e.t, self.bpm),
                on: e.on,
                channel: e.ch,
                note: e.note,
                velocity: e.vel,
            })
            .collect();
        let pad = phrases::from_wire(wire, length_ticks.max(1), self.bpm, false);
        self.phrases[index] = pad;
        let dir = phrases::phrases_dir_from_env();
        let _ = phrases::save_pad(&dir, index, &self.phrases[index], self.bpm);
        outbox.clip_load(
            index as u8,
            self.phrases[index].length_ticks,
            "oneshot",
            self.phrases[index].events.clone(),
        );
        self.pads_selected = index;
        self.status_line = format!("{} recorded", phrases::pad_label(index));
        self.push_log(format!("pad rec {}", phrases::pad_label(index)));
    }

    fn push_pad_rec(&mut self, on: bool, channel: u8, note: u8, velocity: u8) {
        if self.pads_recording.is_none() {
            return;
        }
        let Some(started) = self.pad_rec_started else {
            return;
        };
        self.pad_rec_events.push(PadRecEvent {
            t: started.elapsed().as_secs_f64(),
            on,
            ch: channel & 0x0f,
            note: note & 0x7f,
            vel: if on { velocity.max(1).min(127) } else { 0 },
        });
    }

    fn nudge_pad_gain(&mut self, delta: f32, outbox: &mut Outbox) {
        if !self.pads_edit {
            self.status_line = "switch to EDIT first".into();
            return;
        }
        let index = self.pads_selected.min(15);
        if self.phrases[index].empty {
            self.status_line = format!("{} empty", phrases::pad_label(index));
            return;
        }
        let old = self.phrases[index].gain;
        let new_gain = (old + delta).clamp(0.1, 2.0);
        if (new_gain - old).abs() < 0.001 {
            return;
        }
        // Rescale event velocities relative to gain change.
        let ratio = new_gain / old.max(0.01);
        for ev in &mut self.phrases[index].events {
            if ev.on && ev.velocity > 0 {
                ev.velocity = ((f32::from(ev.velocity) * ratio).round() as u32).clamp(1, 127) as u8;
            }
        }
        self.phrases[index].gain = new_gain;
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
        let dir = phrases::phrases_dir_from_env();
        let _ = phrases::save_pad(&dir, index, &self.phrases[index], self.bpm);
        self.status_line = format!("{} gain {:.1}", phrases::pad_label(index), new_gain);
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
        if index == 0 {
            self.sync_wave_bank();
        }
        self.status_line = format!("{} {:.2}", Self::SYNTH_PARAM_NAMES[index], value);
        self.mark_dirty();
    }

    const FX_PARAM_NAMES: [&'static str; 3] = ["drive", "delay_mix", "reverb_mix"];
    const DRUM_MACRO_NAMES: [&'static str; 4] =
        ["drum_tone", "drum_noise", "drum_pitch", "drum_decay"];
    const DRUM_MACRO_LABELS: [&'static str; 4] = ["TONE", "SNAP", "PITCH", "DECAY"];

    fn apply_fx_slider(&mut self, index: usize, py: i32, outbox: &mut Outbox) {
        if index >= 3 {
            return;
        }
        let track = self.layout.settings_fx_slider(index);
        let y = 1.0 - ((py - track.y) as f32 / track.h.max(1) as f32);
        let value = y.clamp(0.0, 1.0);
        match self.fx_target {
            FxEditTarget::Bus => {
                self.fx_bus[index] = value;
                outbox.fx_bus(Self::FX_PARAM_NAMES[index], value);
                self.status_line = format!("bus {} {:.2}", Self::FX_PARAM_NAMES[index], value);
            }
            FxEditTarget::Voice => {
                self.fx_voice[index] = value;
                outbox.fx_voice(0, Self::FX_PARAM_NAMES[index], value);
                self.status_line = format!("voice0 {} {:.2}", Self::FX_PARAM_NAMES[index], value);
            }
            FxEditTarget::DrumGroup => {
                self.fx_drum[index] = value;
                outbox.fx_drum_group(Self::FX_PARAM_NAMES[index], value);
                self.status_line = format!("drums {} {:.2}", Self::FX_PARAM_NAMES[index], value);
            }
        }
        self.mark_dirty();
    }

    fn cycle_fx_target(&mut self) {
        self.fx_target = match self.fx_target {
            FxEditTarget::Bus => FxEditTarget::Voice,
            FxEditTarget::Voice => FxEditTarget::DrumGroup,
            FxEditTarget::DrumGroup => FxEditTarget::Bus,
        };
        self.status_line = match self.fx_target {
            FxEditTarget::Bus => "FX target: BUS".into(),
            FxEditTarget::Voice => "FX target: VOICE 0".into(),
            FxEditTarget::DrumGroup => "FX target: DRUM GROUP".into(),
        };
        self.mark_dirty();
    }

    fn nudge_drum_macro(&mut self, index: usize, outbox: &mut Outbox) {
        if index >= 4 {
            return;
        }
        let next = (self.drum_macros[index] + 0.15) % 1.05;
        let value = if next > 1.0 { 0.0 } else { next };
        self.drum_macros[index] = value;
        outbox.synth(Self::DRUM_MACRO_NAMES[index], value);
        self.status_line = format!(
            "{} {:.2}",
            Self::DRUM_MACRO_LABELS[index],
            value
        );
    }

    fn nudge_vibrato(&mut self, delta: f32, outbox: &mut Outbox) {
        self.vibrato_always = (self.vibrato_always + delta).clamp(0.0, 1.0);
        outbox.synth("vibrato_always", self.vibrato_always);
        self.status_line = format!("vib {:.2}", self.vibrato_always);
        self.mark_dirty();
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
                let p = kaoss_ui::program_at(self.kaoss_show_all, index);
                self.kaoss_program = KAOSS_PROGRAMS
                    .iter()
                    .position(|q| q.id == p.id)
                    .unwrap_or(0);
                self.status_line = format!("program {}", p.label);
                self.mark_dirty();
            }
            KaossPicker::Scale => {
                self.kaoss_scale_index = (index % jambox_core::KAOSS_SCALES.len()) as u8;
                self.push_kaoss_scale(outbox);
                self.status_line = jambox_core::kaoss_scale(self.kaoss_scale_index as usize)
                    .label
                    .to_string();
                self.mark_dirty();
            }
            KaossPicker::Key => {
                self.kaoss_key = (index % 12) as u8;
                self.push_kaoss_scale(outbox);
                self.status_line =
                    format!("key {}", jambox_core::NOTE_NAMES[self.kaoss_key as usize]);
                self.mark_dirty();
            }
            KaossPicker::Octave => {
                self.kaoss_octaves = ((index % 4) + 1) as u8;
                self.push_kaoss_scale(outbox);
                self.status_line = format!("{} oct", self.kaoss_octaves);
                self.mark_dirty();
            }
            KaossPicker::Gate => {
                self.kaoss_gate = index % kaoss_ui::GATE_PATTERNS.len();
                self.kaoss_gate_t0 = Some(Instant::now());
                self.kaoss_gate_on = false;
                self.status_line = kaoss_ui::gate(self.kaoss_gate).label.to_string();
                self.mark_dirty();
            }
        }
    }

    fn push_kaoss_scale(&mut self, outbox: &mut Outbox) {
        outbox.kaoss_scale(
            self.kaoss_scale_index,
            self.kaoss_key,
            self.kaoss_root_midi(),
            self.kaoss_octaves,
        );
    }

    fn nudge_kaoss_bpm(&mut self, delta: f32, outbox: &mut Outbox) {
        self.bpm = (self.bpm + delta).clamp(40.0, 240.0);
        self.seq.bpm = self.bpm;
        outbox.tempo(self.bpm);
        self.status_line = format!("tempo {:.0}", self.bpm);
        self.mark_dirty();
    }

    fn toggle_kaoss_hold(&mut self, outbox: &mut Outbox) {
        self.kaoss_hold = !self.kaoss_hold;
        if !self.kaoss_hold {
            if let Some(gesture) = self.kaoss_hold_gesture.take() {
                self.kaoss_touch_edge(gesture, TouchPhase::Up, 0.0, 0.0, outbox);
            }
            if !self.kaoss_touching {
                self.release_kaoss_gate(outbox);
            }
            self.kaoss_usb_silence(outbox, true);
            self.status_line = "HOLD off".into();
        } else {
            self.status_line = "HOLD on — latch last pad".into();
        }
        self.mark_dirty();
    }

    fn begin_kaoss_touch(&mut self, gesture: u32, x: f32, y: f32, outbox: &mut Outbox) {
        if let Some(held) = self.kaoss_hold_gesture.take() {
            self.kaoss_touch_edge(held, TouchPhase::Up, 0.0, 0.0, outbox);
        }
        self.release_kaoss_gate(outbox);
        self.kaoss_usb_silence(outbox, true);
        self.kaoss_touching = true;
        self.kaoss_latched_xy = (x, y);
        self.kaoss_gate_t0 = Some(Instant::now());
        self.kaoss_gate_on = false;
        self.push_kaoss_trail(x, y);
        self.push_kaoss_ripple(x, y);
        let prog = kaoss_ui::program(self.kaoss_program);
        let gated = prog.note && kaoss_ui::gate(self.kaoss_gate).beats > 0.0;
        if prog.note && !gated {
            self.kaoss_touch_edge(gesture, TouchPhase::Down, x, y, outbox);
            self.kaoss_usb_note_on(x, y, outbox);
        } else if gated {
            self.kaoss_gate_gesture = Some(gesture);
        }
        self.kaoss_usb_pad_down(x, y, outbox);
        self.apply_kaoss_xy(prog, x, y, outbox);
    }

    fn move_kaoss_touch(&mut self, gesture: u32, x: f32, y: f32, outbox: &mut Outbox) {
        self.kaoss_latched_xy = (x, y);
        self.push_kaoss_trail(x, y);
        let prog = kaoss_ui::program(self.kaoss_program);
        let gated = prog.note && kaoss_ui::gate(self.kaoss_gate).beats > 0.0;
        if prog.note && !gated {
            self.kaoss_touch_edge(gesture, TouchPhase::Move, x, y, outbox);
            self.kaoss_usb_note_follow(x, y, outbox);
        }
        self.kaoss_usb_xy(x, y, outbox);
        self.apply_kaoss_xy(prog, x, y, outbox);
    }

    fn end_kaoss_touch(&mut self, gesture: u32, x: f32, y: f32, outbox: &mut Outbox) {
        self.kaoss_touching = false;
        let prog = kaoss_ui::program(self.kaoss_program);
        let gated = prog.note && kaoss_ui::gate(self.kaoss_gate).beats > 0.0;
        if self.kaoss_hold && prog.note {
            if !gated {
                self.kaoss_hold_gesture = Some(gesture);
            } else {
                self.kaoss_gate_gesture = Some(gesture);
            }
            self.status_line = "HOLD latched".into();
            return;
        }
        if gated {
            self.release_kaoss_gate(outbox);
        } else if prog.note {
            self.kaoss_touch_edge(gesture, TouchPhase::Up, x, y, outbox);
            self.kaoss_usb_note_off(outbox);
        }
        self.kaoss_usb_pad_up(outbox);
    }

    fn release_kaoss_gate(&mut self, outbox: &mut Outbox) {
        if self.kaoss_gate_on {
            let (x, y) = self.kaoss_latched_xy;
            if let Some(g) = self.kaoss_gate_gesture {
                self.kaoss_touch_edge(g, TouchPhase::Up, x, y, outbox);
            }
            self.kaoss_usb_note_off(outbox);
            self.kaoss_gate_on = false;
        }
        self.kaoss_gate_gesture = None;
    }

    fn tick_kaoss_gate(&mut self, outbox: &mut Outbox) {
        let prog = kaoss_ui::program(self.kaoss_program);
        let gate = kaoss_ui::gate(self.kaoss_gate);
        if !prog.note || gate.beats <= 0.0 {
            return;
        }
        let active = self.kaoss_touching || self.kaoss_hold;
        if !active {
            if self.kaoss_gate_on {
                self.release_kaoss_gate(outbox);
            }
            return;
        }
        let period = kaoss_ui::gate_period_sec(gate, self.bpm);
        if period <= 0.0 {
            return;
        }
        let t0 = match self.kaoss_gate_t0 {
            Some(t) => t,
            None => {
                self.kaoss_gate_t0 = Some(Instant::now());
                return;
            }
        };
        let elapsed = t0.elapsed().as_secs_f64();
        let phase = (elapsed % period) / period;
        let want_on = phase < gate.duty;
        let (x, y) = self.kaoss_latched_xy;
        let gesture = match self.kaoss_gate_gesture {
            Some(g) => g,
            None => {
                let g = self.next_gesture;
                self.next_gesture = self.next_gesture.wrapping_add(1).max(1);
                self.kaoss_gate_gesture = Some(g);
                g
            }
        };
        if want_on && !self.kaoss_gate_on {
            self.kaoss_touch_edge(gesture, TouchPhase::Down, x, y, outbox);
            self.kaoss_usb_note_on(x, y, outbox);
            self.kaoss_gate_on = true;
        } else if !want_on && self.kaoss_gate_on {
            self.kaoss_touch_edge(gesture, TouchPhase::Up, x, y, outbox);
            self.kaoss_usb_note_off(outbox);
            self.kaoss_gate_on = false;
        } else if want_on && self.kaoss_gate_on {
            self.kaoss_touch_edge(gesture, TouchPhase::Move, x, y, outbox);
            self.kaoss_usb_note_follow(x, y, outbox);
        }
    }

    fn kaoss_touch_edge(
        &self,
        gesture: u32,
        phase: TouchPhase,
        x: f32,
        y: f32,
        outbox: &mut Outbox,
    ) {
        if self.kaoss_out.includes_local() {
            outbox.touch(gesture, phase, x, y, self.kaoss_channel);
        }
    }

    fn kaoss_root_midi(&self) -> u8 {
        match self.kaoss_octaves {
            1 | 2 => 48,
            3 => 36,
            _ => 24,
        }
    }

    fn kaoss_note_at(&self, x: f32) -> u8 {
        let scale = kaoss_scale(self.kaoss_scale_index as usize);
        let notes = scale_notes(
            scale.degrees,
            self.kaoss_key,
            self.kaoss_root_midi(),
            self.kaoss_octaves,
        );
        let n = notes
            .iter()
            .rposition(|&n| n != 0)
            .map(|i| i + 1)
            .unwrap_or(1);
        note_at_x(x, &notes[..n], n)
    }

    fn midi_cc01(value: f32) -> u8 {
        (value.clamp(0.0, 1.0) * 127.0).round() as u8
    }

    fn kaoss_emit_cc(&mut self, control: u8, value: u8, outbox: &mut Outbox) {
        if !self.kaoss_out.includes_usb() {
            return;
        }
        let changed = match control {
            KAOSS_CC_X => {
                if self.kaoss_cc_x_sent == Some(value) {
                    false
                } else {
                    self.kaoss_cc_x_sent = Some(value);
                    true
                }
            }
            KAOSS_CC_Y => {
                if self.kaoss_cc_y_sent == Some(value) {
                    false
                } else {
                    self.kaoss_cc_y_sent = Some(value);
                    true
                }
            }
            KAOSS_CC_TOUCH => {
                if self.kaoss_cc_touch_sent == Some(value) {
                    false
                } else {
                    self.kaoss_cc_touch_sent = Some(value);
                    true
                }
            }
            _ => true,
        };
        if changed {
            outbox.midi_emit(
                "cc",
                self.kaoss_channel,
                None,
                None,
                Some(control),
                Some(value as u16),
            );
        }
    }

    fn kaoss_usb_pad_down(&mut self, x: f32, y: f32, outbox: &mut Outbox) {
        self.kaoss_emit_cc(KAOSS_CC_TOUCH, 127, outbox);
        self.kaoss_usb_xy(x, y, outbox);
    }

    fn kaoss_usb_pad_up(&mut self, outbox: &mut Outbox) {
        self.kaoss_emit_cc(KAOSS_CC_TOUCH, 0, outbox);
        self.kaoss_cc_x_sent = None;
        self.kaoss_cc_y_sent = None;
    }

    fn kaoss_usb_xy(&mut self, x: f32, y: f32, outbox: &mut Outbox) {
        self.kaoss_emit_cc(KAOSS_CC_X, Self::midi_cc01(x), outbox);
        self.kaoss_emit_cc(KAOSS_CC_Y, Self::midi_cc01(y), outbox);
    }

    fn kaoss_usb_note_on(&mut self, x: f32, y: f32, outbox: &mut Outbox) {
        if !self.kaoss_out.includes_usb() {
            return;
        }
        let note = self.kaoss_note_at(x);
        let velocity = velocity_at_y(y);
        if let Some(old) = self.kaoss_usb_note {
            if old != note {
                outbox.midi_emit(
                    "note_off",
                    self.kaoss_channel,
                    Some(old),
                    Some(0),
                    None,
                    None,
                );
            } else {
                return;
            }
        }
        outbox.midi_emit(
            "note_on",
            self.kaoss_channel,
            Some(note),
            Some(velocity),
            None,
            None,
        );
        self.kaoss_usb_note = Some(note);
    }

    fn kaoss_usb_note_follow(&mut self, x: f32, y: f32, outbox: &mut Outbox) {
        if !self.kaoss_out.includes_usb() {
            return;
        }
        let note = self.kaoss_note_at(x);
        let velocity = velocity_at_y(y);
        match self.kaoss_usb_note {
            Some(old) if old == note => {}
            Some(old) => {
                outbox.midi_emit(
                    "note_off",
                    self.kaoss_channel,
                    Some(old),
                    Some(0),
                    None,
                    None,
                );
                outbox.midi_emit(
                    "note_on",
                    self.kaoss_channel,
                    Some(note),
                    Some(velocity),
                    None,
                    None,
                );
                self.kaoss_usb_note = Some(note);
            }
            None => {
                outbox.midi_emit(
                    "note_on",
                    self.kaoss_channel,
                    Some(note),
                    Some(velocity),
                    None,
                    None,
                );
                self.kaoss_usb_note = Some(note);
            }
        }
    }

    fn kaoss_usb_note_off(&mut self, outbox: &mut Outbox) {
        if let Some(note) = self.kaoss_usb_note.take() {
            if self.kaoss_out.includes_usb() {
                outbox.midi_emit(
                    "note_off",
                    self.kaoss_channel,
                    Some(note),
                    Some(0),
                    None,
                    None,
                );
            }
        }
    }

    fn kaoss_usb_silence(&mut self, outbox: &mut Outbox, clear_touch_cc: bool) {
        self.kaoss_usb_note_off(outbox);
        if clear_touch_cc {
            self.kaoss_usb_pad_up(outbox);
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
            if prog.y_param == "vibrato_always" {
                self.vibrato_always = y;
                outbox.synth("vibrato_always", y);
                self.mark_dirty();
            } else if prog.y_param != "tone" {
                outbox.synth(prog.y_param, y);
                if let Some(i) = Self::synth_param_index(prog.y_param) {
                    self.synth_params[i] = y;
                    if i == 0 {
                        self.sync_wave_bank();
                    }
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
                        if i == 0 {
                            self.sync_wave_bank();
                        }
                    }
                }
            }
            if Self::is_bus_param(prog.y_param) {
                outbox.fx_bus(prog.y_param, y);
            } else if prog.y_param == "vibrato_always" {
                self.vibrato_always = y;
                outbox.synth("vibrato_always", y);
            } else {
                outbox.synth(prog.y_param, y);
                if let Some(i) = Self::synth_param_index(prog.y_param) {
                    self.synth_params[i] = y;
                    if i == 0 {
                        self.sync_wave_bank();
                    }
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

    fn in_kaoss_full_exit_zone(py: i32) -> bool {
        py > SCREEN_H - NAV_H - KAOSS_FULL_EXIT_EDGE_PX
    }

    fn watch_kaoss_full_exit(&mut self, py: i32, touching: bool) {
        if !self.kaoss_full {
            self.kaoss_full_exit_since = None;
            return;
        }
        if !touching {
            self.kaoss_full_exit_since = None;
            return;
        }
        if Self::in_kaoss_full_exit_zone(py) {
            if self.kaoss_full_exit_since.is_none() {
                self.kaoss_full_exit_since = Some(Instant::now());
            }
        } else {
            self.kaoss_full_exit_since = None;
        }
    }

    fn tick_kaoss_full_exit(&mut self) {
        if !self.kaoss_full {
            return;
        }
        let Some(t0) = self.kaoss_full_exit_since else {
            return;
        };
        if t0.elapsed().as_millis() as u64 >= KAOSS_PLAY_EXIT_MS {
            self.leave_kaoss_full();
        }
    }

    fn leave_kaoss_full(&mut self) {
        self.kaoss_full = false;
        self.kaoss_full_exit_since = None;
        self.layout.apply_kaoss_full(false);
        self.status_line = "split pad".into();
        self.mark_dirty();
    }

    fn save_voice_as(&mut self, outbox: &mut Outbox) {
        self.ensure_library_loaded();
        let name_a = self
            .wave_names
            .get(self.morph_a as usize)
            .cloned()
            .unwrap_or_else(|| "a".into());
        let name_b = self
            .wave_names
            .get(self.morph_b as usize)
            .cloned()
            .unwrap_or_else(|| "b".into());
        let morph = self.synth_params[0];
        let drive = self.fx_bus[0];
        let tone = self.synth_params[1];
        let delay_mix = self.fx_bus[1];
        let reverb_mix = self.fx_bus[2];
        let existing = self.wave_names.clone();
        let morph_a = self.morph_a as usize;
        let morph_b = self.morph_b as usize;
        let Some(bank) = self.wave_bank.as_mut() else {
            self.status_line = "SAVE AS failed — no wave bank".into();
            return;
        };
        bank.set_morph_pair(morph_a, morph_b);
        bank.set_morph(morph);
        match voice_bake::save_as(
            bank,
            &existing,
            &name_a,
            &name_b,
            morph,
            drive,
            tone,
            delay_mix,
            reverb_mix,
        ) {
            Ok(baked) => {
                self.wave_names = waves::list_wave_names(&waves::waves_dirs_from_env());
                if !self.wave_names.iter().any(|n| n == &baked.name) {
                    self.wave_names.push(baked.name.clone());
                }
                let idx = self
                    .wave_names
                    .iter()
                    .position(|n| n == &baked.name)
                    .unwrap_or(baked.index);
                self.morph_a = idx as u16;
                self.morph_b = idx as u16;
                self.synth_params[0] = 0.0;
                outbox.morph_pair(self.morph_a, self.morph_b);
                outbox.synth("morph", 0.0);
                self.sync_wave_bank();
                self.synth_pick_a = None;
                self.status_line = format!("saved {}", baked.name);
                self.push_log(format!(
                    "SAVE AS {} → {} + {} (dly={} rvb={})",
                    baked.name,
                    baked
                        .wav_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("?"),
                    baked
                        .fx_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("?"),
                    (delay_mix * 127.0) as i32,
                    (reverb_mix * 127.0) as i32,
                ));
                self.mark_dirty();
            }
            Err(e) => {
                self.status_line = format!("SAVE AS failed: {e}");
                self.push_log(format!("SAVE AS failed: {e}"));
            }
        }
    }

    fn swap_morph_pair(&mut self, outbox: &mut Outbox) {
        std::mem::swap(&mut self.morph_a, &mut self.morph_b);
        self.synth_params[0] = 1.0 - self.synth_params[0];
        outbox.morph_pair(self.morph_a, self.morph_b);
        outbox.synth("morph", self.synth_params[0]);
        self.sync_wave_bank();
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
        self.sync_wave_bank();
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
        self.sync_wave_bank();
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

    fn delete_preset(&mut self, index: usize) {
        let dir = presets::presets_dir_from_env();
        let index = index.min(7);
        if presets::delete_slot(&dir, index) {
            self.preset_occupied[index] = false;
            self.status_line = format!("slot {} deleted", index + 1);
        } else {
            self.status_line = "preset delete failed".into();
        }
    }

    fn factory_reset_synth(&mut self, outbox: &mut Outbox) {
        self.synth_params = [0.5, 0.5, 0.8, 0.05, 0.3];
        self.vibrato_always = 0.0;
        self.morph_a = 0;
        self.morph_b = 1;
        outbox.morph_pair(self.morph_a, self.morph_b);
        outbox.synth("morph", self.synth_params[0]);
        outbox.synth("tone", self.synth_params[1]);
        outbox.synth("level", self.synth_params[2]);
        outbox.synth("attack", self.synth_params[3]);
        outbox.synth("release", self.synth_params[4]);
        outbox.synth("vibrato_always", 0.0);
        self.sync_wave_bank();
        self.status_line = "synth factory defaults".into();
        self.mark_dirty();
    }

    fn toggle_pads_voice(&mut self, outbox: &mut Outbox) {
        if !self.pads_edit {
            self.status_line = "switch to EDIT first".into();
            return;
        }
        let index = self.pads_selected.min(15);
        if self.phrases[index].empty {
            self.status_line = format!("{} empty", phrases::pad_label(index));
            return;
        }
        self.phrases[index].voice_locked = !self.phrases[index].voice_locked;
        if self.phrases[index].voice_locked {
            self.phrases[index].morph_a = self.morph_a;
            self.phrases[index].morph_b = self.morph_b;
            self.phrases[index].morph = self.synth_params[0];
        }
        let dir = phrases::phrases_dir_from_env();
        let _ = phrases::save_pad(&dir, index, &self.phrases[index], self.bpm);
        let _ = outbox;
        self.status_line = if self.phrases[index].voice_locked {
            format!("{} VOICE LOCK", phrases::pad_label(index))
        } else {
            format!("{} VOICE FOLLOW", phrases::pad_label(index))
        };
    }

    fn toggle_pads_local_synth(&mut self) {
        if !self.pads_edit {
            self.status_line = "switch to EDIT first".into();
            return;
        }
        let index = self.pads_selected.min(15);
        self.phrases[index].local_synth = !self.phrases[index].local_synth;
        let dir = phrases::phrases_dir_from_env();
        let _ = phrases::save_pad(&dir, index, &self.phrases[index], self.bpm);
        self.status_line = if self.phrases[index].local_synth {
            format!("{} SYNTH", phrases::pad_label(index))
        } else {
            format!("{} MIDI", phrases::pad_label(index))
        };
    }

    fn wipe_kaoss_fx(&mut self, outbox: &mut Outbox) {
        self.fx_bus = [0.0, 0.0, 0.0];
        for name in Self::FX_PARAM_NAMES {
            outbox.fx_bus(name, 0.0);
        }
        let defaults = session::SessionState::default();
        self.synth_params[0] = defaults.morph;
        self.synth_params[1] = defaults.tone;
        outbox.synth("morph", defaults.morph);
        outbox.synth("tone", defaults.tone);
        self.sync_wave_bank();
        if self.kaoss_hold {
            self.kaoss_hold = false;
            if let Some(gesture) = self.kaoss_hold_gesture.take() {
                self.kaoss_touch_edge(gesture, TouchPhase::Up, 0.0, 0.0, outbox);
            }
            if !self.kaoss_touching {
                self.release_kaoss_gate(outbox);
            }
            self.kaoss_usb_silence(outbox, true);
        }
        self.status_line = "KAOSS FX wiped".into();
        self.push_log("KAOSS FX wiped");
        self.mark_dirty();
    }

    fn cycle_pads_channel(&mut self) {
        if !self.pads_edit {
            self.status_line = "switch to EDIT first".into();
            return;
        }
        let index = self.pads_selected.min(15);
        let cur = self.phrases[index].out_channel;
        let next = if cur < 0 {
            0
        } else if cur >= 15 {
            -1
        } else {
            cur + 1
        };
        self.phrases[index].out_channel = next;
        let dir = phrases::phrases_dir_from_env();
        let _ = phrases::save_pad(&dir, index, &self.phrases[index], self.bpm);
        self.status_line = if next < 0 {
            format!("{} OUT:rec", phrases::pad_label(index))
        } else {
            format!("{} OUT:CH{}", phrases::pad_label(index), next + 1)
        };
    }

    fn save_seq_as_song(&mut self) {
        let Some((events, length_ticks, _)) = self.seq.snapshot() else {
            self.status_line = "SEQ empty — record first".into();
            return;
        };
        let dir = songs::songs_dir_from_env();
        let path = songs::next_seq_export_path(&dir);
        if songs::write_smf_type0(&path, &events, length_ticks, self.bpm) {
            self.song_files = songs::list_songs(&dir);
            if let Some(pos) = self.song_files.iter().position(|p| p == &path) {
                self.song_selected = pos;
                self.song_scroll = pos.saturating_sub(2);
            }
            self.status_line = format!(
                "saved {}",
                path.file_name().and_then(|n| n.to_str()).unwrap_or("seq.mid")
            );
            self.push_log(self.status_line.clone());
        } else {
            self.status_line = "SAVE SEQ failed".into();
        }
    }

    fn delete_selected_song(&mut self) {
        if self.song_selected >= self.song_files.len() {
            self.status_line = "no song selected".into();
            return;
        }
        let path = self.song_files[self.song_selected].clone();
        if songs::delete_song(&path) {
            self.song_files = songs::list_songs(&songs::songs_dir_from_env());
            if self.song_selected >= self.song_files.len() && !self.song_files.is_empty() {
                self.song_selected = self.song_files.len() - 1;
            }
            self.status_line = "song deleted".into();
        } else {
            self.status_line = "song delete failed".into();
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
        let mode = if self.song_loop { "loop" } else { "oneshot" };
        outbox.clip_load(SONG_CLIP_SLOT, length_ticks, mode, events);
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
        let trail = self.kaoss_trail.clone();
        let ripples = self.kaoss_ripples.clone();
        let glow = self.kaoss_viz_glow;
        let hold = self.kaoss_hold;
        for row in 0..LED_ROWS {
            for col in 0..LED_COLS {
                self.cells[row][col] =
                    pad_led_rgb(col, row, t, finger, &trail, &ripples, glow, hold);
            }
        }
    }

    fn push_kaoss_trail(&mut self, x: f32, y: f32) {
        if let Some(last) = self.kaoss_trail.last_mut() {
            let dx = last.0 - x;
            let dy = last.1 - y;
            if dx * dx + dy * dy < 0.0004 {
                last.0 = x;
                last.1 = y;
                last.2 = 1.0;
                return;
            }
        }
        self.kaoss_trail.push((x, y, 1.0));
        if self.kaoss_trail.len() > 12 {
            let drop = self.kaoss_trail.len() - 12;
            self.kaoss_trail.drain(0..drop);
        }
    }

    fn push_kaoss_ripple(&mut self, x: f32, y: f32) {
        self.kaoss_ripples.push((x, y, 0.0));
        if self.kaoss_ripples.len() > 4 {
            let drop = self.kaoss_ripples.len() - 4;
            self.kaoss_ripples.drain(0..drop);
        }
    }

    fn age_kaoss_viz(&mut self, dt: f32) {
        let trail_life = 0.45_f32;
        for p in &mut self.kaoss_trail {
            p.2 -= dt / trail_life;
        }
        self.kaoss_trail.retain(|p| p.2 > 0.0);
        let ripple_life = 0.55_f32;
        for p in &mut self.kaoss_ripples {
            p.2 += dt / ripple_life;
        }
        self.kaoss_ripples.retain(|p| p.2 < 1.0);
    }
}

pub fn pad_led_rgb(
    col: usize,
    row: usize,
    t: f32,
    finger: Option<(f32, f32)>,
    trail: &[(f32, f32, f32)],
    ripples: &[(f32, f32, f32)],
    glow_mode: bool,
    hold: bool,
) -> u32 {
    let base = 0x18u32 + ((col + row) as u32 % 3) * 8;
    let mut r = base;
    let mut g = base + 8;
    let mut b = base + 24;
    if hold {
        r = r.saturating_add(10);
        b = b.saturating_add(14);
    }
    let cx = (col as f32 + 0.5) / LED_COLS as f32;
    let cy = (row as f32 + 0.5) / LED_ROWS as f32;
    if let Some((fx, fy)) = finger {
        let dx = cx - fx;
        let dy = cy - fy;
        let d = (dx * dx + dy * dy).sqrt();
        let radius = if glow_mode { 0.55 } else { 0.40 };
        let falloff = if glow_mode { 1.2 } else { 2.2 };
        let glow = (1.0 - d / radius * falloff / 2.2).clamp(0.0, 1.0);
        let glow = if glow_mode {
            glow.powf(1.1)
        } else {
            glow
        };
        let pulse = 0.65 + 0.35 * (t * 6.0).sin();
        let amp = if glow_mode { 220.0 } else { 180.0 };
        r = (r as f32 + glow * amp * pulse) as u32;
        g = (g as f32 + glow * 40.0) as u32;
        b = (b as f32 + glow * 120.0 * pulse) as u32;
    }
    for &(tx, ty, age) in trail {
        let dx = cx - tx;
        let dy = cy - ty;
        let d = (dx * dx + dy * dy).sqrt();
        let spark = (1.0 - d / 0.22).clamp(0.0, 1.0).powf(1.8) * age.clamp(0.0, 1.0) * 0.55;
        r = (r as f32 + spark * 160.0) as u32;
        g = (g as f32 + spark * 60.0) as u32;
        b = (b as f32 + spark * 100.0) as u32;
    }
    for &(rx, ry, age) in ripples {
        let age = age.clamp(0.0, 1.0);
        let radius = 0.08 + age * 0.72;
        let dx = cx - rx;
        let dy = cy - ry;
        let d = (dx * dx + dy * dy).sqrt();
        let ring = (1.0 - (d - radius).abs() / 0.10).clamp(0.0, 1.0);
        let amp = ring * (1.0 - age) * 0.65;
        r = (r as f32 + amp * 200.0) as u32;
        g = (g as f32 + amp * 120.0) as u32;
        b = (b as f32 + amp * 180.0) as u32;
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

    #[test]
    fn curated_program_pick_stores_absolute_index() {
        let mut model = NativeModel::new();
        model.kaoss_show_all = false;
        model.kaoss_picker = Some(KaossPicker::Program);
        let curated: Vec<_> = KAOSS_PROGRAMS.iter().enumerate().filter(|(_, p)| p.curated).collect();
        assert!(curated.len() >= 2);
        let (abs_idx, prog) = curated[1];
        let mut out = Outbox::new();
        model.apply_kaoss_picker(1, &mut out);
        assert_eq!(model.kaoss_program, abs_idx);
        assert_eq!(KAOSS_PROGRAMS[model.kaoss_program].id, prog.id);
    }

    #[test]
    fn locked_pad_launch_sends_morph_pair() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        model.phrases[0].empty = false;
        model.phrases[0].voice_locked = true;
        model.phrases[0].morph_a = 2;
        model.phrases[0].morph_b = 3;
        model.phrases[0].morph = 0.25;
        model.phrases[0].length_ticks = 960;
        model.toggle_phrase(0, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::MorphPair { a: 2, b: 3 }
        )));
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth { param, value, .. } if param == "morph" && (*value - 0.25).abs() < 1e-6
        )));
        assert!(batch.iter().any(|r| matches!(r, Request::ClipLaunch { slot: 0, .. })));
    }

    #[test]
    fn settings_fx_target_routes_to_voice() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Settings);
        model.fx_target = FxEditTarget::Voice;
        let mut out = Outbox::new();
        let track = model.layout.settings_fx_slider(0);
        model.finger_down(1, track.x + 4, track.y + track.h / 2, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Fx {
                target: jambox_protocol::FxTargetSpec::Voice { index: 0 },
                param,
                ..
            } if param == "drive"
        )));
    }

    #[test]
    fn drum_macro_emits_kit_param() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Synth);
        model.ensure_library_loaded();
        let mut out = Outbox::new();
        let cell = model.layout.synth_macro_cell(0);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth { param, .. } if param == "drum_tone"
        )));
    }
}
