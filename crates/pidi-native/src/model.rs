//! Instrument-surface state. Rendering and IPC consume this; they do not own notes.
//!
//! Remaining Tk gaps (see NATIVE_KIOSK.md): deeper Map remap UI.
//! Map/WIFI/UPDATE are appliance-oriented host hooks.

use crate::chords::{self, ChordSpec, Overlay as ChordsOverlay, QualityRow, PALETTE_SLOTS};
use crate::client::Outbox;
use crate::font::FontStyle;
use crate::host::{self, HostTask};
use crate::kaoss_ui::{self, KaossPicker, KAOSS_PROGRAMS};
use crate::layout::{Hit, Layout, Rect, Surface, NAV_H, SCREEN_H};
use crate::mode::UiMode;
use crate::phrases::{self, PhrasePad};
use crate::presets::{self, PresetSnapshot};
use crate::screensaver;
use crate::scroll::{self, ScrollKind, TOUCH_SCROLL_THRESH_PX};
use crate::seq::{SeqAction, SeqModel, SEQ_CLIP_SLOT};
use crate::session::{self, OutMode, SessionState};
use crate::songs::{self, SONG_CLIP_SLOT};
use crate::voice_bake;
use crate::waves;
use jambox_core::{
    drum_model_for_note, kaoss_scale, note_at_x, scale_notes, velocity_at_y, DrumKit, DrumMacros,
    DrumModel, DRUM_MODEL_COUNT, DRUM_PREVIEW_SAMPLES, DRUM_PREVIEW_SR,
};
use jambox_protocol::{
    MidiNotice, RepeatDivision, RepeatPhase, StatusReply, TouchPhase, WireClipEvent,
};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

pub const LED_COLS: usize = 12;
pub const LED_ROWS: usize = 7;
#[allow(dead_code)]
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
/// KIT sliders: tone / snap(noise) / pitch / decay.
const DEFAULT_DRUM_MACROS: [f32; 4] = [0.60, 0.45, 0.50, 0.55];
const KIT_WAVE_POINTS: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatDivisionChoice {
    Off,
    Quarter,
    Eighth,
    EighthTriplet,
    Sixteenth,
    Triple,
}

impl RepeatDivisionChoice {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Quarter,
        Self::Eighth,
        Self::EighthTriplet,
        Self::Sixteenth,
        Self::Triple,
    ];

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    pub fn is_on(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn as_wire(self) -> RepeatDivision {
        match self {
            Self::Off | Self::Quarter => RepeatDivision::Quarter,
            Self::Eighth => RepeatDivision::Eighth,
            Self::EighthTriplet => RepeatDivision::EighthTriplet,
            Self::Sixteenth => RepeatDivision::Sixteenth,
            Self::Triple => RepeatDivision::QuarterTriplet,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::EighthTriplet => "1/8T",
            Self::Sixteenth => "1/16",
            Self::Triple => "TRIPLE",
        }
    }

    pub fn pad_label(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Quarter => Some("1/4"),
            Self::Eighth => Some("1/8"),
            Self::EighthTriplet => Some("1/8T"),
            Self::Sixteenth => Some("1/16"),
            Self::Triple => Some("TRIP"),
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
    /// GATE ARP phase for this Kaoss contact (independent of other fingers).
    gate_on: bool,
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
            gate_on: false,
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
    /// Per-kit-voice note repeat. Off is a single tap; other values hold-to-repeat.
    pub drum_repeat: [RepeatDivisionChoice; DRUM_MODEL_COUNT],
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
    /// Vibrato depth in semitones (0..2).
    pub vibrato_depth: f32,
    /// Vibrato rate in Hz (1..9).
    pub vibrato_rate: f32,
    pub wave_names: Vec<String>,
    pub wave_bank: Option<jambox_core::WaveBank>,
    pub morph_a: u16,
    pub morph_b: u16,
    /// `Some(true)` = picking A, `Some(false)` = picking B.
    pub synth_pick_a: Option<bool>,
    /// Vibrato depth / rate / always-on overlay (replaces redundant VOICES grid).
    pub synth_vib_open: bool,
    pub synth_pick_scroll: i32,
    /// On-screen keyboard octave relative to C4 (−3 = C1 … +3 = C7).
    pub synth_octave: i8,
    /// Four-operator FM playground: recipe, selected op, per-op knobs, draw matrix.
    pub fm_recipe: usize,
    pub fm_selected: usize,
    pub fm_ops: [jambox_core::FmOpParams; jambox_core::FM_OP_COUNT],
    pub fm_matrix: [[f32; jambox_core::FM_OP_COUNT]; jambox_core::FM_OP_COUNT],
    pub fm_params: [f32; 4],
    pub kaoss_picker_scroll: i32,
    pub log_scroll: usize,
    /// Kit macros per model: tone, noise/snap, pitch, decay.
    pub drum_macros: [[f32; 4]; DRUM_MODEL_COUNT],
    /// Snapshot shown while ALL DRUMS is the edit target.
    pub drum_group_macros: [f32; 4],
    /// Selected kit pad (screen index 0..15 into `PHRASE_GRID_CELLS`).
    pub kit_selected: usize,
    pub kit_all_drums: bool,
    /// WAVE drill-down: sliders + CRT one-shot for the current edit target.
    pub kit_edit_open: bool,
    /// NOTE REPEAT drill-down: none / 1/4 / 1/8 / 1/8T / 1/16 / triple.
    pub kit_repeat_open: bool,
    pub kit_wave: [f32; KIT_WAVE_POINTS],
    kit_wave_dirty: bool,
    pub kaoss_scale_index: u8,
    pub kaoss_key: u8,
    /// Left-edge MIDI of the pad window (C1..C5 typically).
    pub kaoss_root_midi: u8,
    pub kaoss_octaves: u8,
    pub kaoss_full: bool,
    /// Finger parked in the bottom exit strip while full — hold to leave.
    pub kaoss_full_exit_since: Option<Instant>,
    pub kaoss_hold: bool,
    pub kaoss_program: usize,
    pub kaoss_gate: usize,
    pub kaoss_show_all: bool,
    pub kaoss_channel: u8,
    pub kaoss_settings_open: bool,
    pub kaoss_settings_scroll: i32,
    /// Drill-down pad color picker (opened from KAOSS settings COLOR).
    pub kaoss_color_picker_open: bool,
    pub kaoss_show_axis_labels: bool,
    pub kaoss_show_grid_lines: bool,
    pub kaoss_grid_width: i8,
    nav_stack: Vec<UiMode>,
    nav_back_navigating: bool,
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
    pub fx_bus: [f32; 4],
    pub fx_voice: [f32; 4],
    pub fx_drum: [f32; 4],
    pub fx_target: FxEditTarget,
    pub log_lines: Vec<String>,
    pub pads_out: OutMode,
    pub song_out: OutMode,
    pub kaoss_out: OutMode,
    pub chords_out: OutMode,
    pub chords_hold: bool,
    pub chords_key: u8,
    /// Block chords + strumplate octave (−2..+2; 0 = C3 factory).
    pub chords_octave: i8,
    pub chords_arm: bool,
    pub chords_current: Option<ChordSpec>,
    pub chords_palette: [Option<ChordSpec>; PALETTE_SLOTS],
    pub chords_overlay: Option<ChordsOverlay>,
    chords_held: [(Option<u32>, usize, QualityRow); MAX_FINGERS],
    chords_block: [Option<u8>; 4],
    chords_strum_note: Option<u8>,
    pub font_style: FontStyle,
    pub power_menu_open: bool,
    /// SET → UPDATE submenu (Tk-style: local build, CHECK, then INSTALL).
    pub update_panel_open: bool,
    pub update_status: String,
    pub update_busy: bool,
    pub update_available: bool,
    pub update_confirming: bool,
    update_job: Option<std::sync::mpsc::Receiver<host::UpdateCheckResult>>,
    update_job_kind: Option<host::UpdateJobKind>,
    /// SET → WIFI scan/join panel.
    pub wifi_panel_open: bool,
    pub wifi_busy: bool,
    pub wifi_status: String,
    pub wifi_networks: Vec<crate::wifi::WifiNetwork>,
    pub wifi_scroll: usize,
    wifi_job: Option<std::sync::mpsc::Receiver<host::WifiJobResult>>,
    /// Password keyboard overlay (SSID being joined).
    pub wifi_kb_open: bool,
    pub wifi_kb_ssid: String,
    pub wifi_kb_text: String,
    pub wifi_kb_shift: bool,
    pub wifi_kb_sym: bool,
    pub wifi_kb_show: bool,
    mode_before_power: UiMode,
    pub screensaver: screensaver::IdleWatch,
    panel_backlight: screensaver::PanelBacklight,
    pub ui_shift: (i32, i32),
    screensaver_elapsed: f32,
    screensaver_orbit: f32,
    /// Last USB Kaoss note (for retune / release).
    kaoss_usb_note: Option<u8>,
    kaoss_cc_x_sent: Option<u8>,
    kaoss_cc_y_sent: Option<u8>,
    kaoss_cc_touch_sent: Option<u8>,
    pub session_dirty: bool,
    pub last_autosave: Instant,
    /// Pad visualizer: LED cells vs soft radial glow.
    pub kaoss_viz_style: crate::kaoss_viz::KaossVizStyle,
    /// Index into [`crate::kaoss_viz::PAD_COLORS`] (0 = RAINBOW).
    pub kaoss_mono_color: usize,
    /// Peak GLOW envelope across fingers (for HUD / tests).
    pub kaoss_glow_amp: f32,
    /// Per-finger membrane glow (amp + lag shells). Soft-unioned when drawn.
    pub kaoss_glow: [crate::kaoss_viz::GlowTouch; MAX_FINGERS],
    /// Accumulated viz time for wave / pulse animation.
    kaoss_viz_time: f32,
    /// Trail points `(x, y, age)` with age 1 = fresh.
    kaoss_trail: Vec<(f32, f32, f32)>,
    /// Ripples `(x, y, age)` with age 0 = fresh → 1 = gone.
    kaoss_ripples: Vec<(f32, f32, f32)>,
    /// Per-cell touch envelope (CELLS viz) — fades in fast, out slow.
    cell_amp: [[f32; LED_COLS]; LED_ROWS],
    fingers: [Finger; MAX_FINGERS],
    next_gesture: u32,
    cells: [[u32; LED_COLS]; LED_ROWS],
    phrases_loaded: bool,
    session_loaded: bool,
    /// In-flight SET/MAP host subprocess (UPDATE/WIFI/THRU). Polled from tick.
    host_rx: Option<Receiver<(String, Vec<String>)>>,
    host_busy: Option<HostTask>,
}

impl Default for NativeModel {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeModel {
    pub fn new() -> Self {
        let fm_patch = jambox_core::fm_recipe_patch(0);
        let out = Self {
            layout: Layout::new(),
            mode: UiMode::Kaoss,
            drum_repeat: [RepeatDivisionChoice::Off; DRUM_MODEL_COUNT],
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
            vibrato_depth: 0.5,
            vibrato_rate: 5.0,
            wave_names: waves::list_wave_names(&waves::waves_dirs_from_env()),
            wave_bank: None,
            morph_a: 0,
            morph_b: 1,
            synth_pick_a: None,
            synth_vib_open: false,
            synth_pick_scroll: 0,
            synth_octave: 0,
            fm_recipe: 0,
            fm_selected: 3,
            fm_ops: fm_patch.ops,
            fm_matrix: fm_patch.matrix,
            fm_params: [
                fm_patch.ops[3].ratio,
                fm_patch.ops[3].audio,
                fm_patch.ops[3].fold,
                fm_patch.ops[3].env,
            ],
            kaoss_picker_scroll: 0,
            log_scroll: 0,
            drum_macros: [DEFAULT_DRUM_MACROS; DRUM_MODEL_COUNT],
            drum_group_macros: DEFAULT_DRUM_MACROS,
            kit_selected: 4,
            kit_all_drums: false,
            kit_edit_open: false,
            kit_repeat_open: false,
            kit_wave: [0.0; KIT_WAVE_POINTS],
            kit_wave_dirty: true,
            kaoss_scale_index: jambox_core::DEFAULT_KAOSS_SCALE_INDEX,
            kaoss_key: 0,
            kaoss_root_midi: jambox_core::DEFAULT_ROOT_MIDI,
            kaoss_octaves: 2,
            kaoss_full: false,
            kaoss_full_exit_since: None,
            kaoss_hold: false,
            kaoss_program: 0,
            kaoss_gate: 0,
            kaoss_show_all: false,
            kaoss_channel: 0,
            kaoss_settings_open: false,
            kaoss_settings_scroll: 0,
            kaoss_color_picker_open: false,
            kaoss_show_axis_labels: true,
            kaoss_show_grid_lines: true,
            kaoss_grid_width: 2,
            nav_stack: Vec::new(),
            nav_back_navigating: false,
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
            fx_bus: [0.0, 0.0, 0.0, 0.0],
            fx_voice: [0.0, 0.0, 0.0, 0.0],
            fx_drum: [0.0, 0.0, 0.0, 0.0],
            fx_target: FxEditTarget::Bus,
            log_lines: Vec::new(),
            pads_out: OutMode::Both,
            song_out: OutMode::Both,
            kaoss_out: OutMode::Local,
            chords_out: OutMode::Both,
            chords_hold: true,
            chords_key: 0,
            chords_octave: 0,
            chords_arm: false,
            chords_current: Some(ChordSpec::new(0, chords::ChordQuality::Maj)),
            chords_palette: [None; PALETTE_SLOTS],
            chords_overlay: None,
            chords_held: [(None, 0, QualityRow::Maj); MAX_FINGERS],
            chords_block: [None; 4],
            chords_strum_note: None,
            font_style: FontStyle::Retro,
            power_menu_open: false,
            update_panel_open: false,
            update_status: String::new(),
            update_busy: false,
            update_available: false,
            update_confirming: false,
            update_job: None,
            update_job_kind: None,
            wifi_panel_open: false,
            wifi_busy: false,
            wifi_status: String::new(),
            wifi_networks: Vec::new(),
            wifi_scroll: 0,
            wifi_job: None,
            wifi_kb_open: false,
            wifi_kb_ssid: String::new(),
            wifi_kb_text: String::new(),
            wifi_kb_shift: false,
            wifi_kb_sym: false,
            wifi_kb_show: false,
            mode_before_power: UiMode::Kaoss,
            screensaver: screensaver::IdleWatch::new(screensaver::timeout_from_env()),
            panel_backlight: screensaver::PanelBacklight::new(),
            ui_shift: (0, 0),
            screensaver_elapsed: 0.0,
            screensaver_orbit: 0.0,
            kaoss_usb_note: None,
            kaoss_cc_x_sent: None,
            kaoss_cc_y_sent: None,
            kaoss_cc_touch_sent: None,
            session_dirty: false,
            last_autosave: Instant::now(),
            kaoss_viz_style: crate::kaoss_viz::KaossVizStyle::Cells,
            kaoss_mono_color: 0,
            kaoss_glow_amp: 0.0,
            kaoss_glow: [crate::kaoss_viz::GlowTouch::idle(); MAX_FINGERS],
            kaoss_viz_time: 0.0,
            kaoss_trail: Vec::new(),
            kaoss_ripples: Vec::new(),
            cell_amp: [[0.0; LED_COLS]; LED_ROWS],
            fingers: [Finger::silent(); MAX_FINGERS],
            next_gesture: 1,
            cells: [[0; LED_COLS]; LED_ROWS],
            phrases_loaded: false,
            session_loaded: false,
            host_rx: None,
            host_busy: None,
        };
        // Ensure ~/.local/share/pidi/{songs,phrases,…} exist on first boot.
        let _ = crate::paths::data_root();
        out
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
            // Full catalog bank (builtins + wav dirs) so scope A/B indices match the picker.
            let mut bank = waves::load_wave_bank(&waves::waves_dirs_from_env());
            // Prefer bank order (skips files that failed to decode).
            if bank.len() > self.wave_names.len() || self.wave_names.len() <= 4 {
                self.wave_names = bank.names().to_vec();
            }
            let max = bank.len().saturating_sub(1);
            bank.set_morph_pair(
                (self.morph_a as usize).min(max),
                (self.morph_b as usize).min(max),
            );
            bank.set_morph(self.synth_params[0]);
            bank.rebuild_morph();
            self.wave_bank = Some(bank);
        }
        if !self.session_loaded {
            if let Some(outbox) = outbox {
                self.session_loaded = true;
                if let Some(s) = session::load(&session::session_path_from_env()) {
                    self.apply_session(&s, outbox);
                }
            }
        }
        self.panel_backlight.ensure_lit();
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
        self.power_menu_open = false;
        self.update_panel_open = false;
        self.update_job = None;
        self.update_job_kind = None;
        self.update_busy = false;
        self.update_confirming = false;
        self.wifi_panel_open = false;
        self.wifi_kb_open = false;
        self.wifi_job = None;
        self.wifi_busy = false;
        self.mode = mode;
        self.status_line.clear();
        self.kaoss_picker = None;
        self.kaoss_picker_scroll = 0;
        self.kaoss_settings_open = false;
        self.kaoss_settings_scroll = 0;
        self.kaoss_color_picker_open = false;
        self.synth_pick_a = None;
        self.synth_vib_open = false;
        self.synth_pick_scroll = 0;
        self.kit_edit_open = false;
        self.kit_repeat_open = false;
        self.chords_overlay = None;
        self.chords_arm = false;
        if mode == UiMode::Drums {
            self.kit_wave_dirty = true;
        }
        if mode != UiMode::Kaoss && self.kaoss_full {
            self.kaoss_full = false;
            self.kaoss_full_exit_since = None;
            self.layout.apply_kaoss_full(false);
        }
    }

    fn sync_melody_engine(&mut self, outbox: &mut Outbox) {
        if self.mode == UiMode::Fm {
            self.push_fm_params(outbox);
            outbox.synth("fm_enable", 1.0);
            outbox.knob_map("fm");
        } else {
            outbox.synth("fm_enable", 0.0);
            if self.mode == UiMode::Drums {
                outbox.knob_map("drums");
            } else {
                outbox.knob_map("keys");
            }
        }
    }

    fn switch_mode(&mut self, mode: UiMode, outbox: &mut Outbox) {
        if self.mode == UiMode::Kaoss && mode != UiMode::Kaoss {
            self.leave_kaoss_mode(outbox);
        }
        if self.mode == UiMode::Chords && mode != UiMode::Chords {
            self.chords_block_off(outbox);
            self.chords_strum_off(outbox);
        }
        self.set_mode(mode);
        self.sync_melody_engine(outbox);
        if mode == UiMode::Fm {
            self.status_line = format!("FM · {}", jambox_core::fm_recipe(self.fm_recipe).title);
        }
    }

    fn tracks_nav_history(mode: UiMode) -> bool {
        matches!(
            mode,
            UiMode::Home
                | UiMode::Synth
                | UiMode::Fm
                | UiMode::Seq
                | UiMode::Pads
                | UiMode::Kaoss
                | UiMode::Chords
                | UiMode::Songs
                | UiMode::Presets
                | UiMode::Fx
                | UiMode::Log
                | UiMode::Settings
        )
    }

    fn push_nav_history(&mut self, next: UiMode) {
        if self.nav_back_navigating {
            return;
        }
        let prev = self.mode;
        if prev == next || !Self::tracks_nav_history(prev) {
            return;
        }
        if self.nav_stack.last() == Some(&prev) {
            return;
        }
        self.nav_stack.push(prev);
        if self.nav_stack.len() > 16 {
            self.nav_stack.remove(0);
        }
    }

    pub fn can_nav_back(&self) -> bool {
        self.overlay_nav_target().is_some() || !self.nav_stack.is_empty()
    }

    fn overlay_nav_target(&self) -> Option<&'static str> {
        if self.power_menu_open {
            return Some("power");
        }
        if self.update_panel_open {
            return Some("update");
        }
        if self.wifi_kb_open {
            return Some("wifi_kb");
        }
        if self.wifi_panel_open {
            return Some("wifi");
        }
        if self.kaoss_color_picker_open {
            return Some("kaoss_color");
        }
        if self.kaoss_settings_open {
            return Some("kaoss_settings");
        }
        if self.kaoss_picker.is_some() {
            return Some("kaoss_picker");
        }
        if self.synth_pick_a.is_some() {
            return Some("morph");
        }
        if self.synth_vib_open {
            return Some("vib");
        }
        if self.kit_edit_open {
            return Some("kit_edit");
        }
        if self.kit_repeat_open {
            return Some("kit_repeat");
        }
        None
    }

    fn leave_kaoss_mode(&mut self, outbox: &mut Outbox) {
        self.kaoss_settings_open = false;
        self.kaoss_color_picker_open = false;
        self.kaoss_picker = None;
        if self.kaoss_full {
            self.leave_kaoss_full();
        }
        let ids: Vec<i32> = self
            .fingers
            .iter()
            .filter(|f| f.active && f.surface == Surface::Kaoss)
            .map(|f| f.id)
            .collect();
        for id in ids {
            self.finger_up(id, outbox);
        }
        if !self.kaoss_hold && (self.kaoss_gate_on || self.kaoss_touching) {
            self.release_kaoss_gate(outbox);
            self.kaoss_touching = false;
        }
    }

    fn close_kaoss_settings(&mut self) {
        self.kaoss_settings_open = false;
        self.kaoss_settings_scroll = 0;
        self.kaoss_color_picker_open = false;
        self.status_line.clear();
    }

    fn open_kaoss_settings(&mut self) {
        if self.kaoss_settings_open {
            return;
        }
        if self.kaoss_full {
            self.leave_kaoss_full();
        }
        self.kaoss_picker = None;
        self.kaoss_color_picker_open = false;
        self.kaoss_settings_open = true;
        self.kaoss_settings_scroll = 0;
        self.status_line = "KAOSS settings".into();
        self.mark_dirty();
    }

    fn open_kaoss_color_picker(&mut self) {
        self.kaoss_color_picker_open = true;
        self.status_line = "PAD COLOR".into();
        self.mark_dirty();
    }

    fn close_kaoss_color_picker(&mut self) {
        self.kaoss_color_picker_open = false;
        self.status_line = "KAOSS settings".into();
    }

    fn nav_back(&mut self, outbox: &mut Outbox) {
        if !self.can_nav_back() {
            return;
        }
        match self.overlay_nav_target() {
            Some("power") => self.close_power_menu(true),
            Some("update") => self.close_update_panel(),
            Some("wifi_kb") => self.close_wifi_keyboard(),
            Some("wifi") => self.close_wifi_panel(),
            Some("kaoss_color") => self.close_kaoss_color_picker(),
            Some("kaoss_settings") => self.close_kaoss_settings(),
            Some("kaoss_picker") => self.kaoss_picker = None,
            Some("morph") => {
                self.synth_pick_a = None;
                self.synth_pick_scroll = 0;
                self.status_line.clear();
            }
            Some("vib") => {
                self.synth_vib_open = false;
                self.status_line.clear();
            }
            Some("kit_edit") => {
                self.kit_edit_open = false;
                self.status_line.clear();
            }
            Some("kit_repeat") => {
                self.kit_repeat_open = false;
                self.status_line.clear();
            }
            _ => {
                if let Some(prev) = self.nav_stack.pop() {
                    self.nav_back_navigating = true;
                    self.switch_mode(prev, outbox);
                    self.nav_back_navigating = false;
                }
            }
        }
        self.mark_dirty();
    }

    fn mode_from_session(raw: &str) -> UiMode {
        match raw.to_ascii_lowercase().as_str() {
            "home" => UiMode::Home,
            "syn" | "synth" => UiMode::Synth,
            "fm" => UiMode::Fm,
            "kit" | "drums" => UiMode::Drums,
            "seq" => UiMode::Seq,
            "pad" | "pads" => UiMode::Pads,
            "kao" | "kaoss" => UiMode::Kaoss,
            "chd" | "chords" => UiMode::Chords,
            "sng" | "songs" => UiMode::Songs,
            "pre" | "presets" => UiMode::Presets,
            "fx" => UiMode::Fx,
            "map" => UiMode::Map,
            "log" => UiMode::Log,
            "set" | "settings" => UiMode::Settings,
            _ => UiMode::Kaoss,
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

    /// Fill `out` with live Kaoss pad contacts (pad-normalized XY). Returns count.
    pub fn copy_kaoss_fingers(&self, out: &mut [(f32, f32)]) -> usize {
        let mut n = 0;
        for f in &self.fingers {
            if n >= out.len() {
                break;
            }
            if f.active && f.surface == Surface::Kaoss {
                out[n] = (f.x, f.y);
                n += 1;
            }
        }
        n
    }

    /// Peak GLOW envelope across all finger blooms (wash / tests).
    pub fn kaoss_glow_amp(&self) -> f32 {
        self.kaoss_glow
            .iter()
            .map(|g| g.amp)
            .fold(0.0_f32, f32::max)
    }

    /// Pad X for the note currently sounding (live finger or HOLD latch).
    pub fn kaoss_sounding_x(&self) -> Option<f32> {
        if !kaoss_ui::program(self.kaoss_program).note {
            return None;
        }
        if let Some((x, _)) = self.kaoss_finger() {
            return Some(x);
        }
        if self.kaoss_hold_gesture.is_some() || (self.kaoss_hold && self.kaoss_gate_on) {
            return Some(self.kaoss_latched_xy.0);
        }
        None
    }

    pub fn kaoss_viz_time(&self) -> f32 {
        self.kaoss_viz_time
    }

    pub fn kaoss_trail_points(&self) -> &[(f32, f32, f32)] {
        &self.kaoss_trail
    }

    pub fn kaoss_ripple_points(&self) -> &[(f32, f32, f32)] {
        &self.kaoss_ripples
    }

    /// Drop edit-only pad hits while PLAY view is active (rects stay laid out).
    fn filter_hit(&self, hit: Hit) -> Hit {
        if self.mode == UiMode::Pads && !self.pads_edit {
            match hit {
                Hit::PadsClearArm
                | Hit::PadsTrig
                | Hit::PadsModeArm
                | Hit::PadsRec
                | Hit::PadsVolUp
                | Hit::PadsVolDown
                | Hit::PadsVoice
                | Hit::PadsChannel
                | Hit::PadsSynth => Hit::None,
                other => other,
            }
        } else {
            hit
        }
    }

    /// 1 while GATE ARP is in the on phase — drives the LED field pulse.
    pub fn kaoss_gate_flash(&self) -> f32 {
        let prog = kaoss_ui::program(self.kaoss_program);
        if !prog.note {
            return 0.0;
        }
        let gate = kaoss_ui::gate(self.kaoss_gate);
        if gate.beats <= 0.0 || !self.kaoss_touching {
            return 0.0;
        }
        if self.kaoss_gate_on {
            1.0
        } else {
            0.0
        }
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
            if self.kaoss_viz_style.is_cells() {
                self.paint_cells();
            }
            self.tick_kaoss_full_exit();
        }
        self.tick_kaoss_gate(outbox);
        self.tick_screensaver(dt);
        self.poll_host_job();
        self.poll_update_job();
        self.poll_wifi_job();
        if self.mode == UiMode::Drums && self.kit_edit_open && self.kit_wave_dirty {
            self.rebuild_kit_wave();
        }
    }

    pub fn host_busy(&self) -> Option<HostTask> {
        self.host_busy
    }

    fn start_host_job(&mut self, task: HostTask) {
        if self.host_busy.is_some() {
            self.status_line = "busy — wait".into();
            self.push_log("host job already running");
            self.mark_dirty();
            return;
        }
        self.status_line = task.busy_status().into();
        self.push_log(task.busy_status());
        self.host_rx = Some(task.spawn());
        self.host_busy = Some(task);
        self.mark_dirty();
    }

    fn poll_host_job(&mut self) {
        let Some(rx) = self.host_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok((status, lines)) => {
                self.host_rx = None;
                self.host_busy = None;
                self.status_line = status;
                for line in lines {
                    self.push_log(line);
                }
                self.mark_dirty();
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.host_rx = None;
                self.host_busy = None;
                self.status_line = "host job failed".into();
                self.push_log("host worker exited without a result");
                self.mark_dirty();
            }
        }
    }

    pub fn screensaver_active(&self) -> bool {
        self.screensaver.active
    }

    pub fn screensaver_orbit(&self) -> f32 {
        self.screensaver_orbit
    }

    fn tick_screensaver(&mut self, dt: f32) {
        self.screensaver_elapsed += dt;
        if self.screensaver.active {
            self.screensaver_orbit += dt;
            self.ui_shift = (0, 0);
            return;
        }
        self.ui_shift = screensaver::pixel_shift_xy(self.screensaver_elapsed);
        if self.screensaver.due() {
            self.show_screensaver();
        }
    }

    fn open_power_menu(&mut self) {
        if self.power_menu_open {
            return;
        }
        self.mode_before_power = self.mode;
        self.kaoss_picker = None;
        self.kaoss_settings_open = false;
        self.kaoss_color_picker_open = false;
        self.synth_vib_open = false;
        self.synth_pick_a = None;
        self.update_panel_open = false;
        self.update_job = None;
        self.wifi_panel_open = false;
        self.wifi_kb_open = false;
        self.wifi_job = None;
        self.power_menu_open = true;
        self.status_line = "power menu".into();
        self.mark_dirty();
    }

    fn close_power_menu(&mut self, restore_mode: bool) {
        if !self.power_menu_open {
            return;
        }
        self.power_menu_open = false;
        if restore_mode {
            self.mode = self.mode_before_power;
        }
        self.mark_dirty();
    }

    fn open_update_panel(&mut self) {
        self.power_menu_open = false;
        self.wifi_panel_open = false;
        self.wifi_kb_open = false;
        self.wifi_job = None;
        self.update_panel_open = true;
        self.update_busy = false;
        self.update_available = false;
        self.update_confirming = false;
        self.update_job = None;
        self.update_job_kind = None;
        // Local stamp only — never blocks on network.
        self.update_status = format!(
            "{}\n\nRemote: —\nTap CHECK to look at GitHub master.",
            host::update_local_status()
        );
        self.status_line = "Update".into();
        self.mark_dirty();
    }

    fn close_update_panel(&mut self) {
        if !self.update_panel_open {
            return;
        }
        self.update_panel_open = false;
        self.update_busy = false;
        self.update_confirming = false;
        self.update_job = None;
        self.update_job_kind = None;
        self.status_line.clear();
        self.mark_dirty();
    }

    fn open_wifi_panel(&mut self) {
        self.power_menu_open = false;
        self.update_panel_open = false;
        self.wifi_kb_open = false;
        self.wifi_panel_open = true;
        self.wifi_busy = false;
        self.wifi_job = None;
        self.wifi_scroll = 0;
        self.wifi_status = "Tap SCAN to list networks, or REJOIN saved credentials.".into();
        self.status_line = "Wi-Fi".into();
        self.mark_dirty();
        if self.wifi_networks.is_empty() {
            self.start_wifi_scan();
        }
    }

    fn close_wifi_panel(&mut self) {
        self.close_wifi_keyboard();
        if !self.wifi_panel_open {
            return;
        }
        self.wifi_panel_open = false;
        self.wifi_busy = false;
        self.wifi_job = None;
        self.status_line.clear();
        self.mark_dirty();
    }

    fn start_wifi_scan(&mut self) {
        if self.wifi_busy {
            return;
        }
        self.wifi_busy = true;
        self.wifi_status = "Scanning…".into();
        self.wifi_job = Some(host::spawn_wifi_job(host::WifiJobKind::Scan));
        self.status_line = "SCAN…".into();
        self.mark_dirty();
    }

    fn start_wifi_rejoin(&mut self) {
        if self.wifi_busy {
            return;
        }
        self.wifi_busy = true;
        self.wifi_status = "Rejoining…".into();
        self.wifi_job = Some(host::spawn_wifi_job(host::WifiJobKind::Rejoin));
        self.status_line = "REJOIN…".into();
        self.mark_dirty();
    }

    fn start_wifi_join(&mut self, ssid: String, password: String) {
        if self.wifi_busy {
            return;
        }
        self.wifi_busy = true;
        self.wifi_status = format!("Joining {ssid}…");
        self.wifi_job = Some(host::spawn_wifi_job(host::WifiJobKind::Join {
            ssid,
            password,
        }));
        self.status_line = "JOIN…".into();
        self.mark_dirty();
    }

    fn open_wifi_keyboard(&mut self, ssid: String) {
        self.wifi_kb_open = true;
        self.wifi_kb_ssid = ssid;
        self.wifi_kb_text.clear();
        self.wifi_kb_shift = false;
        self.wifi_kb_sym = false;
        self.wifi_kb_show = false;
        self.wifi_status = format!("Enter password for {}", self.wifi_kb_ssid);
        self.mark_dirty();
    }

    fn close_wifi_keyboard(&mut self) {
        if !self.wifi_kb_open {
            return;
        }
        self.wifi_kb_open = false;
        self.wifi_kb_ssid.clear();
        self.wifi_kb_text.clear();
        self.wifi_kb_shift = false;
        self.wifi_kb_sym = false;
        self.wifi_kb_show = false;
        self.mark_dirty();
    }

    fn wifi_scroll_by(&mut self, delta: i32) {
        let max_scroll = self
            .wifi_networks
            .len()
            .saturating_sub(crate::wifi::LIST_VISIBLE);
        let next = (self.wifi_scroll as i32 + delta).clamp(0, max_scroll as i32);
        self.wifi_scroll = next as usize;
        self.mark_dirty();
    }

    fn wifi_select_row(&mut self, row: usize) {
        if self.wifi_busy {
            return;
        }
        let idx = self.wifi_scroll + row;
        let Some(net) = self.wifi_networks.get(idx).cloned() else {
            return;
        };
        if net.is_open() {
            self.start_wifi_join(net.ssid, String::new());
        } else {
            self.open_wifi_keyboard(net.ssid);
        }
    }

    fn handle_wifi_kb_key(&mut self, row: usize, col: usize) {
        let rows = if self.wifi_kb_sym {
            crate::wifi::keyboard_sym_rows()
        } else {
            crate::wifi::keyboard_abc_rows(self.wifi_kb_shift)
        };
        let Some(r) = rows.get(row) else {
            return;
        };
        let Some((_label, action, _span)) = r.get(col) else {
            return;
        };
        match *action {
            "pad" => {}
            "shift" => {
                self.wifi_kb_shift = !self.wifi_kb_shift;
            }
            "sym" => {
                self.wifi_kb_sym = true;
                self.wifi_kb_shift = false;
            }
            "abc" => {
                self.wifi_kb_sym = false;
            }
            "back" => {
                self.wifi_kb_text.pop();
            }
            "space" => {
                self.wifi_kb_text.push(' ');
            }
            "ok" => {
                let ssid = self.wifi_kb_ssid.clone();
                let pass = self.wifi_kb_text.clone();
                self.close_wifi_keyboard();
                self.start_wifi_join(ssid, pass);
                return;
            }
            ch => {
                self.wifi_kb_text.push_str(ch);
                if self.wifi_kb_shift && !self.wifi_kb_sym {
                    self.wifi_kb_shift = false;
                }
            }
        }
        self.mark_dirty();
    }

    fn poll_wifi_job(&mut self) {
        let Some(rx) = self.wifi_job.as_ref() else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        self.wifi_job = None;
        self.wifi_busy = false;
        match result {
            host::WifiJobResult::Scan { networks, error } => {
                self.wifi_networks = networks;
                self.wifi_scroll = 0;
                if error.is_empty() {
                    self.wifi_status = format!("{} networks", self.wifi_networks.len());
                    self.status_line = "scan ok".into();
                } else {
                    self.wifi_status = error.clone();
                    self.status_line = "scan fail".into();
                    self.push_log(error);
                }
            }
            host::WifiJobResult::Rejoin { ok, detail, lines } => {
                for line in lines {
                    self.push_log(line);
                }
                self.wifi_status = detail.clone();
                self.status_line = if ok {
                    format!("WIFI OK {detail}")
                } else {
                    format!("WIFI {detail}")
                };
            }
            host::WifiJobResult::Join { ok, detail, ssid } => {
                self.push_log(detail.clone());
                self.wifi_status = detail.clone();
                self.status_line = if ok {
                    format!("joined {ssid}")
                } else {
                    format!("join fail")
                };
                if ok {
                    self.close_wifi_keyboard();
                }
            }
        }
        self.mark_dirty();
    }

    fn start_update_check(&mut self) {
        if self.update_busy {
            return;
        }
        if self.update_confirming {
            self.update_confirming = false;
            self.update_status = format!(
                "{}\n\nConfirm cancelled.",
                host::update_local_status()
            );
            self.mark_dirty();
            return;
        }
        self.update_busy = true;
        self.update_confirming = false;
        self.update_status = format!(
            "{}\n\nChecking GitHub for the latest master…",
            host::update_local_status()
        );
        self.update_job_kind = Some(host::UpdateJobKind::Check);
        self.update_job = Some(host::spawn_update_job(host::UpdateJobKind::Check));
        self.status_line = "CHECK…".into();
        self.mark_dirty();
    }

    fn start_update_apply(&mut self) {
        if self.update_busy {
            return;
        }
        if !self.update_confirming {
            if !self.update_available {
                // No prior CHECK, or already up to date — run CHECK first.
                self.update_busy = true;
                self.update_status = format!(
                    "{}\n\nChecking before install…",
                    host::update_local_status()
                );
                self.update_job_kind = Some(host::UpdateJobKind::Check);
                self.update_job = Some(host::spawn_update_job(host::UpdateJobKind::Check));
                // When check returns available, arm confirm (see poll_update_job).
                self.status_line = "CHECK…".into();
                self.mark_dirty();
                return;
            }
            self.update_confirming = true;
            self.update_status = format!(
                "{}\n\nThis deploys new code from GitHub, then restarts.\n\
                 Phrases, songs, presets, and settings.json are kept.\n\
                 Tap INSTALL NOW to continue, or CANCEL (CHECK).",
                host::update_local_status()
            );
            self.status_line = "confirm install".into();
            self.mark_dirty();
            return;
        }
        self.update_confirming = false;
        self.update_busy = true;
        self.update_status = "Starting install…\n(UI stays responsive; engines restart when done.)".into();
        self.update_job_kind = Some(host::UpdateJobKind::Apply);
        self.update_job = Some(host::spawn_update_job(host::UpdateJobKind::Apply));
        self.status_line = "INSTALL…".into();
        self.mark_dirty();
    }

    fn poll_update_job(&mut self) {
        let Some(rx) = self.update_job.as_ref() else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        let kind = self.update_job_kind.take();
        self.update_job = None;
        self.update_busy = false;
        for line in &result.lines {
            self.push_log(line.clone());
        }
        match kind {
            Some(host::UpdateJobKind::Check) => {
                self.update_available = result.available;
                let local = host::update_local_status();
                if result.available && self.update_status.contains("Checking before install") {
                    self.update_confirming = true;
                    self.update_status = format!(
                        "{}\n\n{}\n\nTap INSTALL NOW to continue, or CANCEL (CHECK).",
                        local, result.status
                    );
                    self.status_line = "confirm install".into();
                } else {
                    self.update_confirming = false;
                    let remote = if result.available {
                        format!("{}\nUPDATE available.", result.status)
                    } else {
                        result.status.clone()
                    };
                    self.update_status = format!("{local}\n\n{remote}");
                    self.status_line = if result.available {
                        "UPDATE ready".into()
                    } else if result.ok {
                        "up to date".into()
                    } else {
                        "CHECK failed".into()
                    };
                }
            }
            Some(host::UpdateJobKind::Apply) => {
                self.update_available = false;
                self.update_confirming = false;
                self.update_status = result.status.clone();
                self.status_line = if result.ok {
                    "installed".into()
                } else {
                    "INSTALL failed".into()
                };
            }
            None => {}
        }
        self.mark_dirty();
    }

    fn show_screensaver(&mut self) {
        if !self.screensaver.activate() {
            return;
        }
        self.panel_backlight.dim();
        self.close_power_menu(false);
        self.close_update_panel();
        self.close_wifi_panel();
        self.mark_dirty();
    }

    fn wake_screensaver(&mut self) {
        if self.screensaver.poke() {
            self.panel_backlight.restore();
            self.mark_dirty();
        }
    }

    fn run_pi_power(&mut self, action: &str, outbox: &mut Outbox) {
        outbox.panic();
        self.panic_ui_state(outbox);
        let path = session::session_path_from_env();
        let _ = session::save(&path, &self.capture_session());
        let (status, lines) = host::pi_power(action);
        self.status_line = status;
        for line in lines {
            self.push_log(line);
        }
        self.mark_dirty();
    }

    pub fn mark_dirty(&mut self) {
        self.session_dirty = true;
    }

    pub fn apply_session(&mut self, s: &SessionState, outbox: &mut Outbox) {
        self.bpm = s.bpm.clamp(40.0, 240.0);
        self.seq.bpm = self.bpm;
        self.synth_params = [s.morph, s.tone, s.level, s.attack, s.release];
        self.vibrato_always = s.vibrato_always.clamp(0.0, 1.0);
        self.vibrato_depth = s.vibrato_depth.clamp(0.0, 2.0);
        self.vibrato_rate = s.vibrato_rate.clamp(1.0, 9.0);
        self.morph_a = s.morph_a;
        self.morph_b = s.morph_b;
        self.synth_octave = s.synth_octave.clamp(
            crate::layout::Layout::SYNTH_OCTAVE_MIN,
            crate::layout::Layout::SYNTH_OCTAVE_MAX,
        );
        self.kaoss_scale_index = if s.version < 2 {
            jambox_core::migrate_legacy_scale_index(s.kaoss_scale_index)
        } else {
            s.kaoss_scale_index
                .min(jambox_core::KAOSS_SCALES.len() as u8 - 1)
        };
        self.kaoss_key = s.kaoss_key.min(11);
        self.kaoss_root_midi = kaoss_ui::clamp_root_midi(s.kaoss_root_midi);
        self.kaoss_octaves = s.kaoss_octaves.clamp(1, 4);
        self.kaoss_program = s.kaoss_program % KAOSS_PROGRAMS.len();
        self.kaoss_gate = if s.version < 3 {
            kaoss_ui::migrate_legacy_gate_index(s.kaoss_gate)
        } else {
            s.kaoss_gate % kaoss_ui::GATE_PATTERNS.len()
        };
        self.kaoss_hold = s.kaoss_hold;
        self.kaoss_show_all = s.kaoss_show_all;
        self.kaoss_channel = s.kaoss_channel & 0x0f;
        self.fx_bus = [
            s.fx_bus[0],
            s.fx_bus[1],
            s.fx_bus[2],
            s.fx_bus_flanger.clamp(0.0, 1.0),
        ];
        // Voice flange (SYNTH / FX→VOICE); bus flange is independent global wet.
        self.fx_voice[3] = s.fx_flanger.clamp(0.0, 1.0);
        self.pads_out = s.pads_out;
        self.song_out = s.song_out;
        self.kaoss_out = s.kaoss_out;
        self.chords_out = s.chords_out;
        self.chords_hold = s.chords_hold;
        self.chords_key = s.chords_key.min(11);
        self.chords_octave = s.chords_octave.clamp(chords::OCTAVE_MIN, chords::OCTAVE_MAX);
        self.font_style = s.font_style;
        let (style, color) =
            crate::kaoss_viz::load_viz_from_session(&s.kaoss_viz_style, s.kaoss_mono_color);
        self.kaoss_viz_style = style;
        self.kaoss_mono_color = color;
        if s.screensaver_sec >= 0.0 && std::env::var("MIDI_TONE_SCREENSAVER_SEC").is_err() {
            self.screensaver.timeout_sec = s.screensaver_sec;
        }
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
        outbox.synth("vibrato_depth", self.vibrato_depth / 2.0);
        outbox.synth(
            "vibrato_rate",
            ((self.vibrato_rate - 1.0) / 8.0).clamp(0.0, 1.0),
        );
        for (model, macros) in self.drum_macros.iter().enumerate() {
            for (i, name) in Self::DRUM_MACRO_NAMES.iter().enumerate() {
                outbox.synth_drum(name, macros[i], Some(model as u8));
            }
        }
        for (i, name) in ["drive", "delay_mix", "reverb_mix", "flanger_mix"]
            .iter()
            .enumerate()
        {
            outbox.fx_bus(name, self.fx_bus[i]);
        }
        self.push_voice_fx("flanger_mix", self.fx_voice[3], outbox);
        self.sync_wave_bank();
        self.push_kaoss_scale(outbox);
        if !s.mode.is_empty() {
            self.mode = Self::mode_from_session(&s.mode);
        }
        self.sync_melody_engine(outbox);
        self.session_dirty = false;
        self.status_line = "session loaded".into();
    }

    pub fn capture_session(&self) -> SessionState {
        SessionState {
            version: 3,
            bpm: self.bpm,
            morph: self.synth_params[0],
            tone: self.synth_params[1],
            level: self.synth_params[2],
            attack: self.synth_params[3],
            release: self.synth_params[4],
            morph_a: self.morph_a,
            morph_b: self.morph_b,
            synth_octave: self.synth_octave,
            kaoss_scale_index: self.kaoss_scale_index,
            kaoss_key: self.kaoss_key,
            kaoss_root_midi: self.kaoss_root_midi,
            kaoss_octaves: self.kaoss_octaves,
            kaoss_program: self.kaoss_program,
            kaoss_gate: self.kaoss_gate,
            kaoss_hold: self.kaoss_hold,
            fx_bus: [self.fx_bus[0], self.fx_bus[1], self.fx_bus[2]],
            fx_flanger: self.fx_voice[3],
            fx_bus_flanger: self.fx_bus[3],
            kaoss_show_all: self.kaoss_show_all,
            kaoss_channel: self.kaoss_channel,
            vibrato_always: self.vibrato_always,
            vibrato_depth: self.vibrato_depth,
            vibrato_rate: self.vibrato_rate,
            mode: self.mode.label().to_ascii_lowercase(),
            pads_out: self.pads_out,
            song_out: self.song_out,
            kaoss_out: self.kaoss_out,
            chords_out: self.chords_out,
            chords_hold: self.chords_hold,
            chords_key: self.chords_key,
            chords_octave: self.chords_octave,
            font_style: self.font_style,
            screensaver_sec: self.screensaver.timeout_sec,
            kaoss_viz_style: self.kaoss_viz_style.wire().into(),
            kaoss_mono_color: self.kaoss_mono_color,
        }
    }

    pub fn maybe_autosave(&mut self) {
        if !self.session_dirty {
            return;
        }
        if self.last_autosave.elapsed().as_secs_f32() < 2.0 {
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
        if self.screensaver.active {
            self.wake_screensaver();
            return;
        }
        self.screensaver.poke();
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

        let hit = if self.kaoss_color_picker_open {
            let base = self.layout.hit(self.mode, px, py);
            if matches!(
                base,
                Hit::Nav(_) | Hit::NavBack | Hit::Power | Hit::HomeTile(_)
            ) {
                base
            } else if self.layout.content.contains(px, py) {
                self.layout.hit_kaoss_color_picker(px, py)
            } else {
                base
            }
        } else if self.kaoss_settings_open {
            let base = self.layout.hit(self.mode, px, py);
            if matches!(
                base,
                Hit::Nav(_) | Hit::NavBack | Hit::Power | Hit::HomeTile(_)
            ) {
                base
            } else if self.layout.content.contains(px, py) {
                self.layout
                    .hit_kaoss_settings(px, py, self.kaoss_settings_scroll)
            } else {
                base
            }
        } else if let Some(_kind) = self.kaoss_picker {
            let base = self.layout.hit(self.mode, px, py);
            match base {
                Hit::KaossProg
                | Hit::KaossScale
                | Hit::KaossKey
                | Hit::KaossOct
                | Hit::KaossOctUp
                | Hit::KaossOctDown
                | Hit::KaossHold
                | Hit::KaossGate
                | Hit::KaossBpmUp
                | Hit::KaossBpmDown
                | Hit::KaossBpmUp5
                | Hit::KaossBpmDown5
                | Hit::KaossFull
                | Hit::KaossShowAll
                | Hit::KaossChannel
                | Hit::KaossSettings
                | Hit::KaossWipeFx
                | Hit::KaossViz
                | Hit::KaossOut
                | Hit::Drum { .. }
                | Hit::Division(_)
                | Hit::Nav(_)
                | Hit::NavBack
                | Hit::Power => base,
                _ => {
                    if self.layout.kaoss.contains(px, py) {
                        Hit::ScrollArea(ScrollKind::KaossPicker)
                    } else {
                        base
                    }
                }
            }
        } else if self.power_menu_open {
            let base = self.layout.hit(self.mode, px, py);
            if matches!(
                base,
                Hit::Nav(_) | Hit::NavBack | Hit::Power | Hit::HomeTile(_) | Hit::SeqRec
            ) {
                base
            } else {
                self.layout.hit_power_menu(px, py)
            }
        } else if self.update_panel_open {
            let base = self.layout.hit(self.mode, px, py);
            if matches!(
                base,
                Hit::Nav(_) | Hit::NavBack | Hit::Power | Hit::HomeTile(_) | Hit::SeqRec
            ) {
                base
            } else {
                self.layout.hit_update_panel(px, py)
            }
        } else if self.wifi_kb_open {
            let base = self.layout.hit(self.mode, px, py);
            if matches!(
                base,
                Hit::Nav(_) | Hit::NavBack | Hit::Power | Hit::HomeTile(_) | Hit::SeqRec
            ) {
                base
            } else {
                self.layout
                    .hit_wifi_keyboard(px, py, self.wifi_kb_sym, self.wifi_kb_shift)
            }
        } else if self.wifi_panel_open {
            let base = self.layout.hit(self.mode, px, py);
            if matches!(
                base,
                Hit::Nav(_) | Hit::NavBack | Hit::Power | Hit::HomeTile(_) | Hit::SeqRec
            ) {
                base
            } else {
                self.layout.hit_wifi_panel(px, py)
            }
        } else if self.mode == UiMode::Synth
            && (self.synth_pick_a.is_some() || self.synth_vib_open)
        {
            self.layout
                .hit_synth_overlay(px, py, self.synth_vib_open, self.synth_pick_a.is_some())
        } else if self.mode == UiMode::Drums && self.kit_edit_open {
            let base = self.layout.hit(self.mode, px, py);
            if matches!(
                base,
                Hit::Nav(_) | Hit::NavBack | Hit::Power | Hit::HomeTile(_)
            ) {
                base
            } else {
                self.layout.hit_kit_edit(px, py)
            }
        } else if self.mode == UiMode::Drums && self.kit_repeat_open {
            let base = self.layout.hit(self.mode, px, py);
            if matches!(
                base,
                Hit::Nav(_) | Hit::NavBack | Hit::Power | Hit::HomeTile(_)
            ) {
                base
            } else {
                self.layout.hit_kit_repeat(px, py)
            }
        } else if self.mode == UiMode::Songs && self.layout.song_list.contains(px, py) {
            Hit::ScrollArea(ScrollKind::SongList)
        } else if self.mode == UiMode::Log {
            let log_top = crate::layout::HUD_H + 70;
            let log_bottom = log_top + 18 * 10;
            if py >= log_top && py < log_bottom {
                Hit::ScrollArea(ScrollKind::Log)
            } else {
                self.layout.hit(self.mode, px, py)
            }
        } else if self.mode == UiMode::Chords && self.chords_overlay.is_some() {
            self.hit_chords_overlay(px, py)
        } else {
            self.layout.hit(self.mode, px, py)
        };
        let hit = self.filter_hit(hit);

        match hit {
            Hit::NavBack => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nav_back(outbox);
            }
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
                    gate_on: false,
                };
                self.close_power_menu(false);
                self.push_nav_history(mode);
                self.switch_mode(mode, outbox);
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
                    gate_on: false,
                };
                self.push_nav_history(mode);
                self.switch_mode(mode, outbox);
            }
            Hit::Power => {
                self.tap_ui(slot, id, gesture, px, py);
                if self.power_menu_open {
                    self.close_power_menu(true);
                } else {
                    self.open_power_menu();
                }
            }
            Hit::PowerShutdown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.run_pi_power("poweroff", outbox);
            }
            Hit::PowerReboot => {
                self.tap_ui(slot, id, gesture, px, py);
                self.run_pi_power("reboot", outbox);
            }
            Hit::PowerScreenOff => {
                self.tap_ui(slot, id, gesture, px, py);
                self.close_power_menu(true);
                self.show_screensaver();
                self.status_line = "screen off — tap to wake".into();
            }
            Hit::PowerBlankCycle => {
                self.tap_ui(slot, id, gesture, px, py);
                self.screensaver.timeout_sec =
                    screensaver::next_timeout_preset(self.screensaver.timeout_sec);
                self.screensaver.poke();
                let label = screensaver::timeout_label_dynamic(self.screensaver.timeout_sec);
                self.status_line = format!("burn-in guard → {label}");
                self.push_log(self.status_line.clone());
                self.mark_dirty();
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
                    gate_on: false,
                };
                self.watch_kaoss_full_exit(py, true);
                self.begin_kaoss_touch(gesture, x, y, outbox);
            }
            Hit::Drum { index, note } => {
                let repeat = self.drum_repeat_for_note(note);
                if self.mode == UiMode::Drums {
                    self.kit_selected = index;
                    self.kit_all_drums = false;
                    if self.kit_edit_open {
                        self.kit_wave_dirty = true;
                    }
                }
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::Drum {
                        note,
                        repeat: repeat.is_on(),
                    },
                    gate_on: false,
                };
                if repeat.is_on() {
                    outbox.repeat(
                        gesture,
                        RepeatPhase::Down,
                        note,
                        DRUM_CHANNEL,
                        110,
                        repeat.as_wire(),
                    );
                } else {
                    outbox.note_on(DRUM_CHANNEL, note, 110);
                }
                self.seq.push_note(true, DRUM_CHANNEL, note, 110);
                self.push_pad_rec(true, DRUM_CHANNEL, note, 110);
            }
            Hit::KitNoteRepeat => {
                self.tap_ui(slot, id, gesture, px, py);
                self.open_kit_repeat();
            }
            Hit::Division(index) => {
                self.apply_drum_repeat(RepeatDivisionChoice::from_index(index));
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                    gate_on: false,
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
                    gate_on: false,
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
                self.status_line = "pads EDIT — CLEAR/MODE/REC/VOL, or TRIG for loop/1shot".into();
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
                    gate_on: false,
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
            Hit::SynthVib => {
                self.tap_ui(slot, id, gesture, px, py);
                self.open_vib_menu();
            }
            Hit::SynthOctDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_synth_octave(-1);
            }
            Hit::SynthOctUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_synth_octave(1);
            }
            Hit::SynthVibAlways => {
                self.tap_ui(slot, id, gesture, px, py);
                self.toggle_vibrato_always(outbox);
            }
            Hit::SynthVibDepthUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_vibrato_depth(0.10, outbox);
            }
            Hit::SynthVibDepthDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_vibrato_depth(-0.10, outbox);
            }
            Hit::SynthVibRateUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_vibrato_rate(0.5, outbox);
            }
            Hit::SynthVibRateDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_vibrato_rate(-0.5, outbox);
            }
            Hit::DrumMacro(index) | Hit::KitSlider(index) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::KitSlider { index },
                    gate_on: false,
                };
                self.apply_kit_slider(index, py, outbox);
            }
            Hit::KitWave => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kit_repeat_open = false;
                self.open_kit_edit();
            }
            Hit::KitPlay => {
                self.tap_ui(slot, id, gesture, px, py);
                self.audition_selected_drum(outbox);
            }
            Hit::KitAllDrums => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kit_all_drums = true;
                self.drum_group_macros = self.edit_source_macros();
                self.fx_target = FxEditTarget::DrumGroup;
                if self.kit_edit_open {
                    self.kit_wave_dirty = true;
                }
                self.status_line = "ALL DRUMS — sliders reshape the whole kit".into();
                self.mark_dirty();
            }
            Hit::ScrollArea(kind) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::ScrollDrag {
                        kind,
                        start_py: py,
                        scroll_at_start: self.scroll_offset(kind),
                        dragging: false,
                    },
                    gate_on: false,
                };
            }
            Hit::SynthPickDone => {
                self.tap_ui(slot, id, gesture, px, py);
                self.synth_pick_a = None;
                self.synth_vib_open = false;
                self.status_line.clear();
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
                    gate_on: false,
                };
                self.apply_synth_slider(index, px, py, outbox);
            }
            Hit::FmRecipe(index) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.select_fm_recipe(index, outbox);
            }
            Hit::FmOp(index) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::FmGraph { from: index },
                    gate_on: false,
                };
            }
            Hit::FmClear => {
                self.tap_ui(slot, id, gesture, px, py);
                self.clear_fm_links(outbox);
            }
            Hit::FmSlider(index) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::FmSlider { index },
                    gate_on: false,
                };
                self.apply_fm_slider(index, py, outbox);
            }
            Hit::SynthKey { note } => {
                let note = self.transpose_synth_key(note);
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::SynthKey { note },
                    gate_on: false,
                };
                // Two fingers on the same key share one MIDI note — only the
                // first contact sounds it; later ones just claim ownership.
                if !self.synth_note_held_elsewhere(slot, note) {
                    outbox.note_on(0, note, 110);
                    self.seq.push_note(true, 0, note, 110);
                    self.push_pad_rec(true, 0, note, 110);
                }
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
            Hit::KaossOctDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_kaoss_root_octave(-1, outbox);
            }
            Hit::KaossOctUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_kaoss_root_octave(1, outbox);
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
                self.nudge_kaoss_bpm(1.0, outbox);
            }
            Hit::KaossBpmDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_kaoss_bpm(-1.0, outbox);
            }
            Hit::KaossBpmUp5 => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_kaoss_bpm(5.0, outbox);
            }
            Hit::KaossBpmDown5 => {
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
            Hit::KaossShowAll => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_show_all = !self.kaoss_show_all;
                self.status_line = if self.kaoss_show_all {
                    "SHOW ALL programs".into()
                } else {
                    "starter programs".into()
                };
                self.mark_dirty();
            }
            Hit::KaossSettings => {
                self.tap_ui(slot, id, gesture, px, py);
                self.open_kaoss_settings();
            }
            Hit::KaossViz | Hit::KaossVizCells => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_viz_style = crate::kaoss_viz::KaossVizStyle::Cells;
                self.status_line = format!(
                    "PAD VIZ: CELLS · {}",
                    crate::kaoss_viz::pad_color_label(self.kaoss_mono_color)
                );
                self.mark_dirty();
            }
            Hit::KaossVizGlow => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_viz_style = crate::kaoss_viz::KaossVizStyle::Glow;
                self.status_line = format!(
                    "PAD VIZ: GLOW · {}",
                    crate::kaoss_viz::pad_color_label(self.kaoss_mono_color)
                );
                self.mark_dirty();
            }
            Hit::KaossColor => {
                self.tap_ui(slot, id, gesture, px, py);
                self.open_kaoss_color_picker();
            }
            Hit::KaossColorPick(index) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_mono_color = index % crate::kaoss_viz::pad_color_count();
                self.status_line = format!(
                    "PAD COLOR → {}",
                    crate::kaoss_viz::pad_color_label(self.kaoss_mono_color)
                );
                self.mark_dirty();
            }
            Hit::KaossColorDone => {
                self.tap_ui(slot, id, gesture, px, py);
                self.close_kaoss_color_picker();
            }
            Hit::KaossAxes => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_show_axis_labels = !self.kaoss_show_axis_labels;
                self.status_line = if self.kaoss_show_axis_labels {
                    "axis labels ON".into()
                } else {
                    "axis labels OFF".into()
                };
                self.mark_dirty();
            }
            Hit::KaossGridLines => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_show_grid_lines = !self.kaoss_show_grid_lines;
                self.status_line = if self.kaoss_show_grid_lines {
                    "grid lines ON".into()
                } else {
                    "grid lines OFF".into()
                };
                self.mark_dirty();
            }
            Hit::KaossGridWidthUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_grid_width = (self.kaoss_grid_width + 1).clamp(1, 6);
                self.status_line = format!("grid {} px", self.kaoss_grid_width);
                self.mark_dirty();
            }
            Hit::KaossGridWidthDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_grid_width = (self.kaoss_grid_width - 1).clamp(1, 6);
                self.status_line = format!("grid {} px", self.kaoss_grid_width);
                self.mark_dirty();
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
            Hit::KaossChannelPick(ch) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_channel = ch.min(15);
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
            Hit::KaossOutPick(index) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.kaoss_out = match index {
                    0 => OutMode::Local,
                    1 => OutMode::Usb,
                    _ => OutMode::Both,
                };
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
                };
                self.seq.nudge_bpm(-1.0);
                outbox.tempo(self.seq.bpm);
                self.bpm = self.seq.bpm;
                self.status_line = self.seq.status.clone();
                self.mark_dirty();
            }
            Hit::SeqToPad => {
                self.tap_ui(slot, id, gesture, px, py);
                self.arm_seq_to_pad(outbox);
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
                    gate_on: false,
                };
                self.preset_selected = index;
                self.status_line = if self.preset_occupied[index] {
                    format!("slot {} selected — tap LOAD", index + 1)
                } else {
                    format!("slot {} empty", index + 1)
                };
                self.mark_dirty();
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
                    gate_on: false,
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
                    gate_on: false,
                };
                let idx = self.song_scroll + row;
                if idx < self.song_files.len() {
                    self.select_song(idx, outbox);
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
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
                    gate_on: false,
                };
                outbox.all_notes_off();
                self.status_line = "all notes off".into();
                self.push_log("all notes off");
            }
            Hit::SettingsAudio => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::UiTap,
                    gate_on: false,
                };
                // Clear hanging notes, then reopen ALSA after a jack swap.
                outbox.all_notes_off();
                outbox.audio_reopen();
                self.status_line = "audio reopen".into();
                self.push_log("audio reopen");
            }
            Hit::FxTarget => {
                self.tap_ui(slot, id, gesture, px, py);
                self.cycle_fx_target();
            }
            Hit::FxSlider(index) => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::FxSlider { index },
                    gate_on: false,
                };
                self.apply_fx_slider(index, py, outbox);
            }
            Hit::SettingsWifi => {
                self.tap_ui(slot, id, gesture, px, py);
                self.open_wifi_panel();
            }
            Hit::SettingsUpdate => {
                self.tap_ui(slot, id, gesture, px, py);
                self.open_update_panel();
            }
            Hit::WifiClose => {
                self.tap_ui(slot, id, gesture, px, py);
                self.close_wifi_panel();
            }
            Hit::WifiScan => {
                self.tap_ui(slot, id, gesture, px, py);
                self.start_wifi_scan();
            }
            Hit::WifiRejoin => {
                self.tap_ui(slot, id, gesture, px, py);
                self.start_wifi_rejoin();
            }
            Hit::WifiScrollUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.wifi_scroll_by(-(crate::wifi::LIST_VISIBLE as i32));
            }
            Hit::WifiScrollDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.wifi_scroll_by(crate::wifi::LIST_VISIBLE as i32);
            }
            Hit::WifiRow(row) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.wifi_select_row(row);
            }
            Hit::WifiKbCancel => {
                self.tap_ui(slot, id, gesture, px, py);
                self.close_wifi_keyboard();
            }
            Hit::WifiKbShow => {
                self.tap_ui(slot, id, gesture, px, py);
                self.wifi_kb_show = !self.wifi_kb_show;
                self.mark_dirty();
            }
            Hit::WifiKbKey { row, col } => {
                self.tap_ui(slot, id, gesture, px, py);
                self.handle_wifi_kb_key(row, col);
            }
            Hit::UpdateClose => {
                self.tap_ui(slot, id, gesture, px, py);
                self.close_update_panel();
            }
            Hit::UpdateCheck => {
                self.tap_ui(slot, id, gesture, px, py);
                self.start_update_check();
            }
            Hit::UpdateApply => {
                self.tap_ui(slot, id, gesture, px, py);
                self.start_update_apply();
            }
            Hit::SettingsFont => {
                self.tap_ui(slot, id, gesture, px, py);
                self.font_style = self.font_style.cycle();
                self.status_line = format!("UI font → {}", self.font_style.label());
                self.mark_dirty();
            }
            Hit::SettingsLog => {
                self.tap_ui(slot, id, gesture, px, py);
                self.push_nav_history(UiMode::Log);
                self.switch_mode(UiMode::Log, outbox);
            }
            Hit::SettingsMap => {
                self.tap_ui(slot, id, gesture, px, py);
                self.push_nav_history(UiMode::Map);
                self.switch_mode(UiMode::Map, outbox);
            }
            Hit::MapThruOn => {
                self.tap_ui(slot, id, gesture, px, py);
                self.start_host_job(HostTask::MapThruOn);
            }
            Hit::MapThruOff => {
                self.tap_ui(slot, id, gesture, px, py);
                self.start_host_job(HostTask::MapThruOff);
            }
            Hit::MapRefresh => {
                self.tap_ui(slot, id, gesture, px, py);
                self.start_host_job(HostTask::MapList);
            }
            Hit::ChordsOut => {
                self.tap_ui(slot, id, gesture, px, py);
                self.chords_out = self.chords_out.cycle();
                self.status_line = format!("chords {}", self.chords_out.label());
                self.mark_dirty();
            }
            Hit::ChordsHold => {
                self.tap_ui(slot, id, gesture, px, py);
                self.chords_hold = !self.chords_hold;
                if !self.chords_hold && !self.chords_contact_held() {
                    // Leaving HOLD with nothing pressed: silence a latched block.
                    self.chords_block_off(outbox);
                }
                self.status_line = if self.chords_hold {
                    "HOLD on".into()
                } else {
                    "HOLD off".into()
                };
                self.mark_dirty();
            }
            Hit::ChordsKey => {
                self.tap_ui(slot, id, gesture, px, py);
                self.chords_overlay = Some(ChordsOverlay::Key);
                self.status_line = "pick key".into();
            }
            Hit::ChordsChanges => {
                self.tap_ui(slot, id, gesture, px, py);
                self.chords_overlay = Some(ChordsOverlay::Changes);
                self.status_line = "pick changes".into();
            }
            Hit::ChordsArm => {
                self.tap_ui(slot, id, gesture, px, py);
                self.chords_arm = !self.chords_arm;
                self.status_line = if self.chords_arm {
                    "ARM: tap a palette pad to store".into()
                } else {
                    "ARM off".into()
                };
            }
            Hit::ChordsOctDown => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_chords_octave(-1, outbox);
            }
            Hit::ChordsOctUp => {
                self.tap_ui(slot, id, gesture, px, py);
                self.nudge_chords_octave(1, outbox);
            }
            Hit::ChordsKeyPick(pc) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.chords_key = pc.min(11);
                self.chords_overlay = None;
                self.status_line = format!("key {}", chords::KEY_NAMES[self.chords_key as usize]);
                self.mark_dirty();
            }
            Hit::ChordsChangesPick(index) => {
                self.tap_ui(slot, id, gesture, px, py);
                self.load_changes(index);
                self.chords_overlay = None;
            }
            Hit::ChordsOverlayClose => {
                self.tap_ui(slot, id, gesture, px, py);
                self.chords_overlay = None;
            }
            Hit::ChordsButton { col, row } => {
                if let Some(qrow) = QualityRow::from_index(row) {
                    self.fingers[slot] = Finger {
                        active: true,
                        id,
                        gesture,
                        x: 0.0,
                        y: 0.0,
                        px,
                        py,
                        surface: Surface::ChordsButton { col, row },
                        gate_on: false,
                    };
                    self.chords_press_button(gesture, col, qrow, outbox);
                }
            }
            Hit::ChordsStrum { y } => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y,
                    px,
                    py,
                    surface: Surface::ChordsStrum,
                    gate_on: false,
                };
                self.chords_strum_to(y, outbox);
            }
            Hit::ChordsPalette { slot: pal } => {
                // ARM / empty-store stay as taps. Playing a filled slot tracks the
                // finger so MOM can release on lift (HOLD keeps the latch).
                if self.chords_arm || self.chords_palette[pal].is_none() {
                    self.tap_ui(slot, id, gesture, px, py);
                    self.chords_palette_tap(pal, outbox);
                } else {
                    self.fingers[slot] = Finger {
                        active: true,
                        id,
                        gesture,
                        x: 0.0,
                        y: 0.0,
                        px,
                        py,
                        surface: Surface::ChordsPalette { slot: pal },
                        gate_on: false,
                    };
                    self.chords_palette_press(pal, outbox);
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
            Surface::FmSlider { index } => {
                self.apply_fm_slider(index, py, outbox);
            }
            Surface::FmGraph { .. } => {}
            Surface::KitSlider { index } => {
                self.apply_kit_slider(index, py, outbox);
            }
            Surface::FxSlider { index } => {
                self.apply_fx_slider(index, py, outbox);
            }
            Surface::ChordsStrum => {
                let (_x, y) = self.layout.chords_strum_play().pad_xy(px, py);
                self.fingers[slot].y = y;
                self.chords_strum_to(y, outbox);
            }
            Surface::SynthKey { note } => {
                if let Some(raw) = self.layout.synth_keyboard_note_at(px, py) {
                    let new_note = self.transpose_synth_key(raw);
                    if new_note != note {
                        // Glissando must not kill a note another finger still holds
                        // (lift smear often parks the rising finger on the neighbor).
                        if !self.synth_note_held_elsewhere(slot, note) {
                            outbox.note_off(0, note);
                            self.seq.push_note(false, 0, note, 0);
                            self.push_pad_rec(false, 0, note, 0);
                        }
                        if !self.synth_note_held_elsewhere(slot, new_note) {
                            outbox.note_on(0, new_note, 110);
                            self.seq.push_note(true, 0, new_note, 110);
                            self.push_pad_rec(true, 0, new_note, 110);
                        }
                        self.fingers[slot].surface = Surface::SynthKey { note: new_note };
                    }
                }
            }
            Surface::ScrollDrag {
                kind,
                start_py,
                scroll_at_start,
                dragging,
            } => {
                let dy = (start_py - py).abs();
                if !dragging && dy >= TOUCH_SCROLL_THRESH_PX {
                    self.fingers[slot].surface = Surface::ScrollDrag {
                        kind,
                        start_py,
                        scroll_at_start,
                        dragging: true,
                    };
                }
                if dragging || dy >= TOUCH_SCROLL_THRESH_PX {
                    self.apply_scroll_drag(kind, start_py, py, scroll_at_start);
                }
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
                self.end_kaoss_touch(finger.gesture, finger.x, finger.y, finger.gate_on, outbox);
            }
            Surface::Drum { note, repeat } => {
                if repeat {
                    outbox.repeat(
                        finger.gesture,
                        RepeatPhase::Up,
                        note,
                        DRUM_CHANNEL,
                        110,
                        self.drum_repeat_for_note(note).as_wire(),
                    );
                } else {
                    outbox.note_off(DRUM_CHANNEL, note);
                }
                self.seq.push_note(false, DRUM_CHANNEL, note, 0);
                self.push_pad_rec(false, DRUM_CHANNEL, note, 0);
            }
            Surface::SynthKey { note } => {
                // Finger already cleared above; remaining holders keep the note.
                if !self.synth_note_held_elsewhere(slot, note) {
                    outbox.note_off(0, note);
                    self.seq.push_note(false, 0, note, 0);
                    self.push_pad_rec(false, 0, note, 0);
                }
            }
            Surface::FmGraph { from } => {
                self.finish_fm_draw(from, finger.px, finger.py, outbox);
            }
            Surface::ChordsButton { col, row } => {
                if let Some(qrow) = QualityRow::from_index(row) {
                    self.chords_release_button(finger.gesture, col, qrow, outbox);
                }
            }
            Surface::ChordsPalette { .. } => {
                self.chords_palette_release(outbox);
            }
            Surface::ChordsStrum => {
                self.chords_strum_off(outbox);
            }
            Surface::ScrollDrag {
                kind,
                start_py,
                dragging,
                ..
            } => {
                let scrolled = dragging || (finger.py - start_py).abs() >= TOUCH_SCROLL_THRESH_PX;
                if !scrolled {
                    self.resolve_scroll_tap(kind, finger.px, finger.py, outbox);
                }
            }
            Surface::Phrase { .. }
            | Surface::SynthSlider { .. }
            | Surface::FmSlider { .. }
            | Surface::KitSlider { .. }
            | Surface::FxSlider { .. }
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
        self.status_line = format!(
            "{} → {}",
            phrases::pad_label(index),
            mode.to_ascii_uppercase()
        );
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

    fn arm_seq_to_pad(&mut self, outbox: &mut Outbox) {
        if self.seq.snapshot().is_none() {
            self.status_line = "SEQ empty — nothing to assign".into();
            self.seq_to_pad_armed = false;
            return;
        }
        self.seq_to_pad_armed = true;
        self.pads_edit = true;
        self.push_nav_history(UiMode::Pads);
        self.switch_mode(UiMode::Pads, outbox);
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
        let _started = self.pad_rec_started.take();
        let events = std::mem::take(&mut self.pad_rec_events);
        if events.is_empty() {
            self.status_line = format!("{} REC empty — dropped", phrases::pad_label(index));
            return;
        }
        let take: Vec<crate::seq::RecEvent> = events
            .iter()
            .map(|e| crate::seq::RecEvent {
                t: e.t,
                on: e.on,
                channel: e.ch,
                note: e.note,
                velocity: e.vel,
            })
            .collect();
        let (trimmed, length_secs) = crate::seq::trim_loop_take(&take, 0.35, 0.05, 2.0);
        if trimmed.is_empty() || length_secs <= 0.0 {
            self.status_line = format!("{} REC empty — dropped", phrases::pad_label(index));
            return;
        }
        let trimmed = crate::seq::close_open_notes(trimmed, length_secs);
        let length_ticks = phrases::seconds_to_ticks(length_secs, self.bpm);
        let wire: Vec<WireClipEvent> = trimmed
            .iter()
            .map(|e| WireClipEvent {
                tick: phrases::seconds_to_ticks(e.t, self.bpm),
                on: e.on,
                channel: e.channel,
                note: e.note,
                velocity: e.velocity,
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

    const SYNTH_PARAM_NAMES: [&'static str; 5] = ["morph", "tone", "level", "attack", "release"];


    fn apply_synth_slider(&mut self, index: usize, _px: i32, py: i32, outbox: &mut Outbox) {
        let track = self.layout.synth_slider(index);
        let y = 1.0 - ((py - track.y) as f32 / track.h.max(1) as f32);
        let value = y.clamp(0.0, 1.0);
        if index == 5 {
            self.fx_voice[3] = value;
            self.push_voice_fx("flanger_mix", value, outbox);
            self.status_line = format!("voice flange {:.2}", value);
            self.mark_dirty();
            return;
        }
        if index >= 5 {
            return;
        }
        self.synth_params[index] = value;
        outbox.synth(Self::SYNTH_PARAM_NAMES[index], value);
        if index == 0 {
            self.sync_wave_bank();
        }
        self.status_line = format!("{} {:.2}", Self::SYNTH_PARAM_NAMES[index], value);
        self.mark_dirty();
    }

    const FM_PARAM_NAMES: [&'static str; 4] = ["fm_clang", "fm_tail", "fm_bright", "fm_hit"];
    const FM_PARAM_LABELS: [&'static str; 4] = ["RATIO", "OUT", "FOLD", "ENV"];

    pub fn fm_patch(&self) -> jambox_core::FmPatch {
        jambox_core::FmPatch {
            ops: self.fm_ops,
            matrix: self.fm_matrix,
        }
    }

    pub fn fm_drag(&self) -> Option<(usize, i32, i32)> {
        self.fingers.iter().find_map(|f| {
            if !f.active {
                return None;
            }
            match f.surface {
                Surface::FmGraph { from } => Some((from, f.px, f.py)),
                _ => None,
            }
        })
    }

    fn sync_fm_sliders_from_op(&mut self) {
        let op = self.fm_ops[self.fm_selected];
        self.fm_params = [op.ratio, op.audio, op.fold, op.env];
    }

    fn apply_fm_patch(&mut self, index: usize) {
        let patch = jambox_core::fm_recipe_patch(index);
        self.fm_recipe = index % jambox_core::FM_RECIPE_COUNT;
        self.fm_ops = patch.ops;
        self.fm_matrix = patch.matrix;
        self.fm_selected = 3;
        self.sync_fm_sliders_from_op();
    }

    fn push_fm_params(&self, outbox: &mut Outbox) {
        outbox.synth("fm_recipe", self.fm_recipe as f32);
        outbox.synth("fm_clear", 1.0);
        for src in 0..jambox_core::FM_OP_COUNT {
            for dst in 0..jambox_core::FM_OP_COUNT {
                let amount = self.fm_matrix[src][dst];
                if amount > 0.02 {
                    outbox.synth("fm_connect", jambox_core::pack_fm_link(src, dst, amount));
                }
            }
        }
        for i in 0..jambox_core::FM_OP_COUNT {
            outbox.synth("fm_op", i as f32);
            let op = self.fm_ops[i];
            outbox.synth("fm_clang", op.ratio);
            outbox.synth("fm_tail", op.audio);
            outbox.synth("fm_bright", op.fold);
            outbox.synth("fm_hit", op.env);
        }
        outbox.synth("fm_op", self.fm_selected as f32);
    }

    fn select_fm_recipe(&mut self, index: usize, outbox: &mut Outbox) {
        self.apply_fm_patch(index);
        self.push_fm_params(outbox);
        let rec = jambox_core::fm_recipe(self.fm_recipe);
        self.status_line = format!("FM · {}", rec.title);
        self.mark_dirty();
    }

    fn select_fm_op(&mut self, index: usize, outbox: &mut Outbox) {
        self.fm_selected = index % jambox_core::FM_OP_COUNT;
        self.sync_fm_sliders_from_op();
        outbox.synth("fm_op", self.fm_selected as f32);
        self.status_line = format!("op {}", jambox_core::OP_NAMES[self.fm_selected]);
        self.mark_dirty();
    }

    fn connect_fm(&mut self, from: usize, to: usize, amount: f32, outbox: &mut Outbox) {
        let from = from % jambox_core::FM_OP_COUNT;
        let to = to % jambox_core::FM_OP_COUNT;
        let amount = amount.clamp(0.25, 0.95);
        self.fm_matrix[from][to] = amount;
        outbox.synth("fm_connect", jambox_core::pack_fm_link(from, to, amount));
        self.status_line = if from == to {
            format!("{} feedback", jambox_core::OP_NAMES[from])
        } else {
            format!(
                "{} → {}",
                jambox_core::OP_NAMES[from],
                jambox_core::OP_NAMES[to]
            )
        };
        self.mark_dirty();
    }

    fn clear_fm_links(&mut self, outbox: &mut Outbox) {
        self.fm_matrix = [[0.0; jambox_core::FM_OP_COUNT]; jambox_core::FM_OP_COUNT];
        outbox.synth("fm_clear", 1.0);
        self.status_line = "links cleared".into();
        self.mark_dirty();
    }

    fn finish_fm_draw(&mut self, from: usize, px: i32, py: i32, outbox: &mut Outbox) {
        const TAP_PX: f32 = 24.0;
        let (sx, sy) = self.layout.fm_op_center(from);
        let dist = ((px - sx) as f32).hypot((py - sy) as f32);
        if let Some(to) = self.layout.fm_op_hit(px, py) {
            if dist >= TAP_PX {
                if self.fm_selected != from {
                    self.select_fm_op(from, outbox);
                }
                self.connect_fm(from, to, 0.7, outbox);
                return;
            }
        }
        self.select_fm_op(from, outbox);
    }

    fn apply_fm_slider(&mut self, index: usize, py: i32, outbox: &mut Outbox) {
        if index >= 4 {
            return;
        }
        let track = self.layout.fm_slider(index);
        let y = 1.0 - ((py - track.y) as f32 / track.h.max(1) as f32);
        let value = y.clamp(0.0, 1.0);
        self.fm_params[index] = value;
        let op = &mut self.fm_ops[self.fm_selected];
        match index {
            0 => op.ratio = value,
            1 => op.audio = value,
            2 => op.fold = value,
            _ => op.env = value,
        }
        outbox.synth(Self::FM_PARAM_NAMES[index], value);
        self.status_line = if index == 0 {
            format!("RATIO {}", jambox_core::clang_label(value))
        } else {
            format!("{} {:.2}", Self::FM_PARAM_LABELS[index], value)
        };
        self.mark_dirty();
    }

    const FX_PARAM_NAMES: [&'static str; 4] = ["drive", "delay_mix", "reverb_mix", "flanger_mix"];
    const DRUM_MACRO_NAMES: [&'static str; 4] =
        ["drum_tone", "drum_noise", "drum_pitch", "drum_decay"];
    const DRUM_MACRO_LABELS: [&'static str; 4] = ["TONE", "SNAP", "PITCH", "DECAY"];

    fn selected_kit_note(&self) -> u8 {
        let cell = phrases::PHRASE_GRID_CELLS[self.kit_selected.min(15)];
        phrases::mpk_note_for_phrase_cell(cell)
    }

    pub fn selected_drum_model(&self) -> DrumModel {
        drum_model_for_note(self.selected_kit_note())
    }

    fn edit_source_macros(&self) -> [f32; 4] {
        self.drum_macros[self.selected_drum_model().index()]
    }

    pub fn edit_drum_macros(&self) -> [f32; 4] {
        if self.kit_all_drums {
            self.drum_group_macros
        } else {
            self.edit_source_macros()
        }
    }

    pub fn drum_repeat_for_note(&self, note: u8) -> RepeatDivisionChoice {
        self.drum_repeat[drum_model_for_note(note).index()]
    }

    pub fn edit_drum_repeat(&self) -> RepeatDivisionChoice {
        if self.kit_all_drums {
            self.drum_repeat[0]
        } else {
            self.drum_repeat[self.selected_drum_model().index()]
        }
    }

    pub fn edit_drum_repeat_mixed(&self) -> bool {
        self.kit_all_drums && self.drum_repeat.iter().any(|d| *d != self.drum_repeat[0])
    }

    pub fn note_repeat_button_label(&self) -> String {
        if self.edit_drum_repeat_mixed() {
            "NOTE REPEAT: MIXED".into()
        } else {
            format!("NOTE REPEAT: {}", self.edit_drum_repeat().label())
        }
    }

    fn open_kit_edit(&mut self) {
        self.kit_repeat_open = false;
        self.kit_edit_open = true;
        self.kit_wave_dirty = true;
        let name = if self.kit_all_drums {
            "ALL DRUMS".to_string()
        } else {
            self.selected_drum_model().name().replace('_', " ")
        };
        self.status_line = format!("{name} — sliders reshape this voice");
        self.mark_dirty();
    }

    fn open_kit_repeat(&mut self) {
        self.kit_edit_open = false;
        self.kit_repeat_open = true;
        let name = if self.kit_all_drums {
            "ALL DRUMS".to_string()
        } else {
            self.selected_drum_model().name().replace('_', " ")
        };
        self.status_line = format!("{name} — note repeat");
        self.mark_dirty();
    }

    fn apply_drum_repeat(&mut self, choice: RepeatDivisionChoice) {
        if self.kit_all_drums {
            self.drum_repeat = [choice; DRUM_MODEL_COUNT];
            self.status_line = format!("ALL DRUMS note repeat {}", choice.label());
        } else {
            let model = self.selected_drum_model().index();
            self.drum_repeat[model] = choice;
            self.status_line = format!(
                "{} note repeat {}",
                self.selected_drum_model().name().replace('_', " "),
                choice.label()
            );
        }
        self.kit_repeat_open = false;
        self.mark_dirty();
    }

    fn audition_selected_drum(&mut self, outbox: &mut Outbox) {
        let note = self.selected_kit_note();
        outbox.note_on(DRUM_CHANNEL, note, 110);
        outbox.note_off(DRUM_CHANNEL, note);
        self.status_line = format!(
            "play {}",
            self.selected_drum_model().name().replace('_', " ")
        );
        self.mark_dirty();
    }

    fn apply_kit_slider(&mut self, index: usize, py: i32, outbox: &mut Outbox) {
        if index >= 4 {
            return;
        }
        let track = self.layout.kit_edit_slider(index);
        let y = 1.0 - ((py - track.y) as f32 / track.h.max(1) as f32);
        let value = y.clamp(0.0, 1.0);
        let name = Self::DRUM_MACRO_NAMES[index];
        if self.kit_all_drums {
            self.drum_group_macros[index] = value;
            for pad in self.drum_macros.iter_mut() {
                pad[index] = value;
            }
            outbox.synth(name, value);
            self.status_line = format!("ALL DRUMS {} {:.2}", Self::DRUM_MACRO_LABELS[index], value);
        } else {
            let model = self.selected_drum_model().index();
            self.drum_macros[model][index] = value;
            outbox.synth_drum(name, value, Some(model as u8));
            self.status_line = format!(
                "{} {} {:.2}",
                self.selected_drum_model().name().replace('_', " "),
                Self::DRUM_MACRO_LABELS[index],
                value
            );
        }
        self.kit_wave_dirty = true;
        self.mark_dirty();
    }

    fn rebuild_kit_wave(&mut self) {
        let m = self.edit_drum_macros();
        let macros = DrumMacros {
            tone: m[0],
            noise: m[1],
            pitch: m[2],
            decay: m[3],
        };
        let mut buf = [0.0f32; DRUM_PREVIEW_SAMPLES];
        DrumKit::preview(
            self.selected_drum_model(),
            macros,
            DRUM_PREVIEW_SR,
            7,
            &mut buf,
        );
        downsample_wave(&buf, &mut self.kit_wave);
        self.kit_wave_dirty = false;
        self.mark_dirty();
    }

    fn apply_fx_slider(&mut self, index: usize, py: i32, outbox: &mut Outbox) {
        if index >= 4 {
            return;
        }
        let track = self.layout.settings_fx_slider(index);
        let y = 1.0 - ((py - track.y) as f32 / track.h.max(1) as f32);
        let value = y.clamp(0.0, 1.0);
        let name = Self::FX_PARAM_NAMES[index];
        match self.fx_target {
            FxEditTarget::Bus => {
                self.fx_bus[index] = value;
                outbox.fx_bus(name, value);
                self.status_line = format!("bus {name} {:.2}", value);
            }
            FxEditTarget::Voice => {
                self.fx_voice[index] = value;
                self.push_voice_fx(name, value, outbox);
                self.status_line = format!("voice {name} {:.2}", value);
            }
            FxEditTarget::DrumGroup => {
                self.fx_drum[index] = value;
                outbox.fx_drum_group(name, value);
                self.status_line = format!("drums {name} {:.2}", value);
            }
        }
        self.mark_dirty();
    }

    /// Apply an insert FX param to both morph endpoints so flange survives the 50% nearer flip.
    fn push_voice_fx(&mut self, name: &str, value: f32, outbox: &mut Outbox) {
        let a = self.morph_a;
        let b = self.morph_b;
        outbox.fx_voice(a, name, value);
        if b != a {
            outbox.fx_voice(b, name, value);
        }
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

    fn nudge_vibrato_depth(&mut self, delta_semis: f32, outbox: &mut Outbox) {
        self.vibrato_depth = (self.vibrato_depth + delta_semis).clamp(0.0, 2.0);
        outbox.synth("vibrato_depth", self.vibrato_depth / 2.0);
        // Tk: bumping depth with wheel-gate off arms always-on so it is audible.
        if self.vibrato_always < 0.01 && delta_semis > 0.0 {
            self.vibrato_always = 1.0;
            outbox.synth("vibrato_always", 1.0);
        }
        self.status_line = format!("vib depth {:.2} st", self.vibrato_depth);
        self.mark_dirty();
    }

    fn nudge_vibrato_rate(&mut self, delta_hz: f32, outbox: &mut Outbox) {
        self.vibrato_rate = (self.vibrato_rate + delta_hz).clamp(1.0, 9.0);
        outbox.synth(
            "vibrato_rate",
            ((self.vibrato_rate - 1.0) / 8.0).clamp(0.0, 1.0),
        );
        self.status_line = format!("vib rate {:.1} Hz", self.vibrato_rate);
        self.mark_dirty();
    }

    fn toggle_vibrato_always(&mut self, outbox: &mut Outbox) {
        self.vibrato_always = if self.vibrato_always > 0.01 { 0.0 } else { 1.0 };
        outbox.synth("vibrato_always", self.vibrato_always);
        self.status_line = if self.vibrato_always > 0.01 {
            "Vibrato ON (screen control)".into()
        } else {
            "Vibrato follows mod wheel".into()
        };
        self.mark_dirty();
    }

    fn push_vibrato_params(&self, outbox: &mut Outbox) {
        outbox.synth("vibrato_always", self.vibrato_always);
        outbox.synth("vibrato_depth", self.vibrato_depth / 2.0);
        outbox.synth(
            "vibrato_rate",
            ((self.vibrato_rate - 1.0) / 8.0).clamp(0.0, 1.0),
        );
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
            gate_on: false,
        };
    }

    fn toggle_kaoss_picker(&mut self, kind: KaossPicker) {
        self.kaoss_picker = match self.kaoss_picker {
            Some(open) if open == kind => None,
            _ => Some(kind),
        };
        self.kaoss_picker_scroll = 0;
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
                let prev = kaoss_ui::program(self.kaoss_program);
                let p = kaoss_ui::program_at(self.kaoss_show_all, index);
                self.kaoss_program = KAOSS_PROGRAMS
                    .iter()
                    .position(|q| q.id == p.id)
                    .unwrap_or(0);
                if prev.y_param == "pitch_bend" && p.y_param != "pitch_bend" {
                    self.reset_kaoss_pitch_bend(outbox);
                }
                if prev.y_param == "tone_lfo" && p.y_param != "tone_lfo" {
                    self.reset_kaoss_tone_lfo(outbox);
                }
                // HOLD drone survives program changes — reassert XY so FILTER/FLANGE
                // immediately ride the latched lead without requiring a fresh press.
                if self.kaoss_hold
                    && (self.kaoss_hold_gesture.is_some()
                        || self.kaoss_gate_on
                        || self.kaoss_touching)
                {
                    let (x, y) = self.kaoss_latched_xy;
                    self.apply_kaoss_xy(p, x, y, outbox);
                }
                self.status_line = format!("program {}", p.label);
                self.mark_dirty();
            }
            KaossPicker::Scale => {
                let s = kaoss_ui::scale_at(self.kaoss_show_all, index);
                self.kaoss_scale_index = jambox_core::kaoss_scale_index_by_id(s.id);
                self.push_kaoss_scale(outbox);
                self.status_line = s.label.to_string();
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
                if index < kaoss_ui::ROOT_OCTAVE_MIDI.len() {
                    self.kaoss_root_midi = kaoss_ui::ROOT_OCTAVE_MIDI[index];
                    self.push_kaoss_scale(outbox);
                    self.status_line =
                        format!("start {}", kaoss_ui::midi_note_label(self.kaoss_root_midi));
                } else {
                    self.kaoss_octaves = ((index - kaoss_ui::ROOT_OCTAVE_MIDI.len()) % 4) as u8 + 1;
                    self.push_kaoss_scale(outbox);
                    self.status_line = format!("{} oct wide", self.kaoss_octaves);
                }
                // Keep picker open so you can set start and width together.
                self.kaoss_picker = Some(KaossPicker::Octave);
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
        // Whole-BPM steps so ±1 can land on 120 after coarse song tempos, etc.
        self.bpm = (self.bpm.round() + delta).clamp(40.0, 240.0);
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
            if !self.kaoss_touching {
                let prog = kaoss_ui::program(self.kaoss_program);
                if prog.y_param == "pitch_bend" {
                    self.reset_kaoss_pitch_bend(outbox);
                }
                if prog.y_param == "tone_lfo" {
                    self.reset_kaoss_tone_lfo(outbox);
                }
            }
            self.status_line = "HOLD off".into();
        } else {
            self.status_line = "HOLD on — latch last pad".into();
        }
        self.mark_dirty();
    }

    fn kaoss_active_count(&self) -> usize {
        self.fingers
            .iter()
            .filter(|f| f.active && f.surface == Surface::Kaoss)
            .count()
    }

    fn begin_kaoss_touch(&mut self, gesture: u32, x: f32, y: f32, outbox: &mut Outbox) {
        // Count excludes this contact only if it isn't registered yet — callers
        // always arm the Finger slot before invoking us, so subtract one.
        let others = self.kaoss_active_count().saturating_sub(1);
        let prog = kaoss_ui::program(self.kaoss_program);
        // HOLD + FX program: keep the latched lead drone so FILTER/FLANGE can
        // audition on top of it (Tk `_kaoss_apply_program` keep_hold path).
        let keep_hold_drone = self.kaoss_hold
            && !prog.note
            && (self.kaoss_hold_gesture.is_some() || self.kaoss_gate_on);
        if others == 0 && !keep_hold_drone {
            if let Some(held) = self.kaoss_hold_gesture.take() {
                self.kaoss_touch_edge(held, TouchPhase::Up, 0.0, 0.0, outbox);
            }
            // Fresh pad press: clear prior gate latch + USB note. Extra fingers
            // must not steal/kill voices already sounding under GATE.
            self.release_kaoss_gate(outbox);
            self.kaoss_usb_silence(outbox, true);
            self.kaoss_gate_t0 = Some(Instant::now());
            self.kaoss_gate_on = false;
        }
        self.kaoss_touching = true;
        self.kaoss_latched_xy = (x, y);
        self.push_kaoss_trail(x, y);
        let gated = prog.note && kaoss_ui::gate(self.kaoss_gate).beats > 0.0;
        if prog.note && !gated {
            self.kaoss_touch_edge(gesture, TouchPhase::Down, x, y, outbox);
            self.record_kaoss_note(true, x, y);
            self.kaoss_usb_note_on(x, y, outbox);
        } else if gated {
            // Shared clock; this gesture joins on the next tick. Mark ownership
            // so HOLD can latch the last contact.
            self.kaoss_gate_gesture = Some(gesture);
            if let Some(slot) = self
                .fingers
                .iter()
                .position(|f| f.active && f.gesture == gesture)
            {
                self.fingers[slot].gate_on = false;
            }
        }
        if others == 0 {
            self.kaoss_usb_pad_down(x, y, outbox);
        } else {
            self.kaoss_usb_xy(x, y, outbox);
        }
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
        } else if gated {
            // While the shared gate is in the on phase, slide this voice.
            if let Some(slot) = self
                .fingers
                .iter()
                .position(|f| f.active && f.gesture == gesture)
            {
                if self.fingers[slot].gate_on {
                    self.kaoss_touch_edge(gesture, TouchPhase::Move, x, y, outbox);
                    self.kaoss_usb_note_follow(x, y, outbox);
                }
            } else if self.kaoss_hold
                && self.kaoss_gate_on
                && self.kaoss_gate_gesture == Some(gesture)
            {
                self.kaoss_touch_edge(gesture, TouchPhase::Move, x, y, outbox);
                self.kaoss_usb_note_follow(x, y, outbox);
            }
        }
        self.kaoss_usb_xy(x, y, outbox);
        self.apply_kaoss_xy(prog, x, y, outbox);
    }

    fn end_kaoss_touch(
        &mut self,
        gesture: u32,
        x: f32,
        y: f32,
        was_gate_on: bool,
        outbox: &mut Outbox,
    ) {
        // Finger slot already cleared by caller — remaining Kaoss contacts only.
        let remaining = self.kaoss_active_count();
        let prog = kaoss_ui::program(self.kaoss_program);
        let gated = prog.note && kaoss_ui::gate(self.kaoss_gate).beats > 0.0;
        if self.kaoss_hold && remaining == 0 {
            if prog.note {
                if !gated {
                    self.kaoss_hold_gesture = Some(gesture);
                    self.kaoss_latched_xy = (x, y);
                } else {
                    // Preserve on-phase across the latch so HOLD doesn't click off.
                    self.kaoss_gate_gesture = Some(gesture);
                    self.kaoss_latched_xy = (x, y);
                    self.kaoss_gate_on = was_gate_on;
                }
                self.kaoss_touching = false;
                self.status_line = "HOLD latched".into();
                return;
            }
            // Switched to FILTER/FLANGE (etc.) while the finger was still down:
            // keep the sounding drone under HOLD and remember ownership.
            if self.kaoss_hold_gesture.is_none() && !gated {
                self.kaoss_hold_gesture = Some(gesture);
            }
            self.kaoss_latched_xy = (x, y);
            self.kaoss_touching = false;
            self.status_line = "HOLD latched".into();
            return;
        }
        if gated {
            if was_gate_on {
                self.kaoss_touch_edge(gesture, TouchPhase::Up, x, y, outbox);
                self.record_kaoss_note(false, x, y);
            }
            if remaining == 0 {
                self.kaoss_usb_note_off(outbox);
                self.kaoss_gate_on = false;
                if !self.kaoss_hold {
                    self.kaoss_gate_gesture = None;
                }
            } else if self.kaoss_gate_gesture == Some(gesture) {
                // Point ownership at another live contact for HOLD/status.
                self.kaoss_gate_gesture = self
                    .fingers
                    .iter()
                    .find(|f| f.active && f.surface == Surface::Kaoss)
                    .map(|f| f.gesture);
            }
        } else if prog.note {
            self.kaoss_touch_edge(gesture, TouchPhase::Up, x, y, outbox);
            self.record_kaoss_note(false, x, y);
            if remaining == 0 {
                self.kaoss_usb_note_off(outbox);
            }
        }
        self.kaoss_touching = remaining > 0;
        if remaining == 0 {
            self.kaoss_usb_pad_up(outbox);
            // Leave bend at center when the pad is idle (HOLD keeps the latched Y).
            if !self.kaoss_hold && prog.y_param == "pitch_bend" {
                self.reset_kaoss_pitch_bend(outbox);
            }
            if !self.kaoss_hold && prog.y_param == "tone_lfo" {
                self.reset_kaoss_tone_lfo(outbox);
            }
        }
    }

    fn release_kaoss_gate(&mut self, outbox: &mut Outbox) {
        let (x, y) = self.kaoss_latched_xy;
        for slot in 0..MAX_FINGERS {
            if self.fingers[slot].active
                && self.fingers[slot].surface == Surface::Kaoss
                && self.fingers[slot].gate_on
            {
                let g = self.fingers[slot].gesture;
                let fx = self.fingers[slot].x;
                let fy = self.fingers[slot].y;
                self.kaoss_touch_edge(g, TouchPhase::Up, fx, fy, outbox);
                self.fingers[slot].gate_on = false;
            }
        }
        if self.kaoss_gate_on {
            if let Some(g) = self.kaoss_gate_gesture {
                // Avoid double-Up if that gesture was already cleared above.
                let already = self
                    .fingers
                    .iter()
                    .any(|f| f.active && f.gesture == g && f.surface == Surface::Kaoss);
                if !already {
                    self.kaoss_touch_edge(g, TouchPhase::Up, x, y, outbox);
                }
            }
            self.kaoss_usb_note_off(outbox);
            self.kaoss_gate_on = false;
        }
        if self.kaoss_active_count() == 0 {
            self.kaoss_gate_gesture = None;
        }
    }

    fn tick_kaoss_gate(&mut self, outbox: &mut Outbox) {
        let prog = kaoss_ui::program(self.kaoss_program);
        let gate = kaoss_ui::gate(self.kaoss_gate);
        if !prog.note || gate.beats <= 0.0 {
            return;
        }
        let live: Vec<(u32, f32, f32, usize)> = self
            .fingers
            .iter()
            .enumerate()
            .filter(|(_, f)| f.active && f.surface == Surface::Kaoss)
            .map(|(i, f)| (f.gesture, f.x, f.y, i))
            .collect();
        let hold_only = live.is_empty() && self.kaoss_hold;
        if live.is_empty() && !hold_only {
            if self.kaoss_gate_on || self.fingers.iter().any(|f| f.gate_on) {
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

        if hold_only {
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
                self.record_kaoss_note(true, x, y);
                self.kaoss_usb_note_on(x, y, outbox);
                self.kaoss_gate_on = true;
            } else if !want_on && self.kaoss_gate_on {
                self.kaoss_touch_edge(gesture, TouchPhase::Up, x, y, outbox);
                self.record_kaoss_note(false, x, y);
                self.kaoss_usb_note_off(outbox);
                self.kaoss_gate_on = false;
            } else if want_on && self.kaoss_gate_on {
                self.kaoss_touch_edge(gesture, TouchPhase::Move, x, y, outbox);
                self.kaoss_usb_note_follow(x, y, outbox);
            }
            return;
        }

        // Polyphonic: every live Kaoss contact shares the gate clock.
        let mut any_on = false;
        let mut usb_xy = self.kaoss_latched_xy;
        for (gesture, x, y, slot) in live {
            let was = self.fingers[slot].gate_on;
            if want_on && !was {
                self.kaoss_touch_edge(gesture, TouchPhase::Down, x, y, outbox);
                self.record_kaoss_note(true, x, y);
                self.fingers[slot].gate_on = true;
                usb_xy = (x, y);
            } else if !want_on && was {
                self.kaoss_touch_edge(gesture, TouchPhase::Up, x, y, outbox);
                self.record_kaoss_note(false, x, y);
                self.fingers[slot].gate_on = false;
            } else if want_on && was {
                self.kaoss_touch_edge(gesture, TouchPhase::Move, x, y, outbox);
                usb_xy = (x, y);
            }
            any_on |= self.fingers[slot].gate_on;
        }
        // USB MIDI out stays monophonic (last/primary XY) so CC/note don't thrash.
        if want_on && any_on {
            if !self.kaoss_gate_on {
                self.kaoss_usb_note_on(usb_xy.0, usb_xy.1, outbox);
            } else {
                self.kaoss_usb_note_follow(usb_xy.0, usb_xy.1, outbox);
            }
            self.kaoss_gate_on = true;
        } else if self.kaoss_gate_on {
            self.kaoss_usb_note_off(outbox);
            self.kaoss_gate_on = false;
        }
    }

    fn record_kaoss_note(&mut self, on: bool, x: f32, y: f32) {
        if !self.kaoss_out.includes_local() {
            return;
        }
        if !kaoss_ui::program(self.kaoss_program).note {
            return;
        }
        let note = self.kaoss_note_at(x);
        let velocity = if on {
            if kaoss_ui::program(self.kaoss_program).y_param == "pitch_bend" {
                100
            } else {
                velocity_at_y(y)
            }
        } else {
            0
        };
        self.seq
            .push_note(on, self.kaoss_channel, note, velocity);
        self.push_pad_rec(on, self.kaoss_channel, note, velocity);
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
            // BEND owns Y for pitch bend — send midline Y so engine velocity stays full.
            let touch_y = if kaoss_ui::program(self.kaoss_program).y_param == "pitch_bend" {
                0.5
            } else {
                y
            };
            outbox.touch(gesture, phase, x, touch_y, self.kaoss_channel);
        }
    }

    fn kaoss_root_midi(&self) -> u8 {
        kaoss_ui::clamp_root_midi(self.kaoss_root_midi)
    }

    fn nudge_kaoss_root_octave(&mut self, step: i8, outbox: &mut Outbox) {
        let next = (self.kaoss_root_midi() as i16 + step as i16 * 12).clamp(
            kaoss_ui::ROOT_OCTAVE_MIDI[0] as i16,
            kaoss_ui::ROOT_OCTAVE_MIDI[kaoss_ui::ROOT_OCTAVE_MIDI.len() - 1] as i16,
        ) as u8;
        if next == self.kaoss_root_midi() {
            return;
        }
        self.kaoss_root_midi = next;
        self.push_kaoss_scale(outbox);
        self.status_line = format!(
            "kaoss {} · {} oct",
            kaoss_ui::midi_note_label(self.kaoss_root_midi),
            self.kaoss_octaves
        );
        self.mark_dirty();
    }

    /// MIDI note under pad X (scale + key + octave span).
    pub fn kaoss_note_at(&self, x: f32) -> u8 {
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
        let velocity = if kaoss_ui::program(self.kaoss_program).y_param == "pitch_bend" {
            100
        } else {
            velocity_at_y(y)
        };
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
        let velocity = if kaoss_ui::program(self.kaoss_program).y_param == "pitch_bend" {
            100
        } else {
            velocity_at_y(y)
        };
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
        // Engine touch path only handles pitch; Y (and FX X) are driven here so
        // LEAD/MORPH/VIB/BEND each own their axis without fighting a hardwired tone map.
        if prog.note {
            if prog.y_param == "tone" {
                outbox.synth("tone", y);
                self.synth_params[1] = y;
                self.mark_dirty();
            } else if prog.y_param == "vib" {
                // Tk parity: Y raises depth and gates always-on vibrato.
                self.vibrato_depth = (y * 2.0).clamp(0.0, 2.0);
                self.vibrato_always = if y > 0.02 { 1.0 } else { 0.0 };
                outbox.synth("vibrato_depth", y.clamp(0.0, 1.0));
                outbox.synth("vibrato_always", self.vibrato_always);
                self.mark_dirty();
            } else if prog.y_param == "tone_lfo" {
                // X plays the scale; Y is how fast the tone filter wobbles.
                outbox.synth("tone_lfo_rate", y.clamp(0.0, 1.0));
                outbox.synth("tone_lfo_amount", 1.0);
                self.mark_dirty();
            } else if prog.y_param == "pitch_bend" {
                self.apply_kaoss_pitch_bend(y, outbox);
            } else {
                self.apply_named_param(prog.y_param, y, outbox);
                self.mark_dirty();
            }
        } else {
            if let Some(xp) = prog.x_param {
                self.apply_named_param(xp, x, outbox);
            }
            if prog.y_param == "vib" {
                self.vibrato_depth = (y * 2.0).clamp(0.0, 2.0);
                self.vibrato_always = if y > 0.02 { 1.0 } else { 0.0 };
                outbox.synth("vibrato_depth", y.clamp(0.0, 1.0));
                outbox.synth("vibrato_always", self.vibrato_always);
            } else if prog.y_param == "tone_lfo" {
                outbox.synth("tone_lfo_rate", y.clamp(0.0, 1.0));
                outbox.synth("tone_lfo_amount", 1.0);
            } else {
                self.apply_named_param(prog.y_param, y, outbox);
            }
            self.mark_dirty();
        }
    }

    fn apply_named_param(&mut self, name: &str, value: f32, outbox: &mut Outbox) {
        let v = value.clamp(0.0, 1.0);
        if Self::is_voice_fx_param(name) {
            if name == "flanger_mix" {
                self.fx_voice[3] = v;
            }
            self.push_voice_fx(name, v, outbox);
            return;
        }
        if Self::is_bus_param(name) {
            outbox.fx_bus(name, v);
            return;
        }
        outbox.synth(name, v);
        if let Some(i) = Self::synth_param_index(name) {
            self.synth_params[i] = v;
            if i == 0 {
                self.sync_wave_bank();
            }
        }
    }

    fn apply_kaoss_pitch_bend(&mut self, y: f32, outbox: &mut Outbox) {
        let semis = kaoss_ui::y_to_pitch_bend_semis(y);
        outbox.synth("pitch_bend", semis);
        if self.kaoss_out.includes_usb() {
            let wheel = kaoss_ui::y_to_pitch_bend_midi(y);
            outbox.midi_emit(
                "pitch_bend",
                self.kaoss_channel,
                None,
                None,
                None,
                Some(wheel),
            );
        }
        self.mark_dirty();
    }

    fn reset_kaoss_pitch_bend(&mut self, outbox: &mut Outbox) {
        outbox.synth("pitch_bend", 0.0);
        if self.kaoss_out.includes_usb() {
            outbox.midi_emit(
                "pitch_bend",
                self.kaoss_channel,
                None,
                None,
                None,
                Some(8192),
            );
        }
    }

    fn reset_kaoss_tone_lfo(&mut self, outbox: &mut Outbox) {
        outbox.synth("tone_lfo_amount", 0.0);
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

    fn is_voice_fx_param(name: &str) -> bool {
        matches!(
            name,
            "flanger_mix" | "flanger_rate" | "flanger_depth" | "flanger_fb"
        )
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

    fn synth_key_base(&self) -> u8 {
        (60i16 + self.synth_octave as i16 * 12).clamp(24, 108) as u8
    }

    /// Map a C4-relative keyboard note (60..71) onto the selected octave.
    fn transpose_synth_key(&self, c4_note: u8) -> u8 {
        let deg = c4_note.saturating_sub(Layout::SYNTH_KEY_BASE);
        self.synth_key_base().saturating_add(deg).min(127)
    }

    /// True when another active synth-key contact still owns `note`.
    fn synth_note_held_elsewhere(&self, except_slot: usize, note: u8) -> bool {
        self.fingers.iter().enumerate().any(|(i, f)| {
            i != except_slot
                && f.active
                && matches!(f.surface, Surface::SynthKey { note: n } if n == note)
        })
    }

    fn nudge_synth_octave(&mut self, delta: i8) {
        let next =
            (self.synth_octave + delta).clamp(Layout::SYNTH_OCTAVE_MIN, Layout::SYNTH_OCTAVE_MAX);
        if next == self.synth_octave {
            return;
        }
        self.synth_octave = next;
        self.status_line = format!("keyboard {}", Self::c_note_label(self.synth_key_base()));
        self.mark_dirty();
    }

    fn c_note_label(midi: u8) -> String {
        let octave = (midi as i32 / 12) - 1;
        format!("C{octave}")
    }

    fn open_synth_pick(&mut self, pick_a: bool) {
        self.synth_vib_open = false;
        self.synth_pick_a = Some(pick_a);
        self.synth_pick_scroll = 0;
        self.status_line = if pick_a {
            "pick wave A · drag to scroll".into()
        } else {
            "pick wave B · drag to scroll".into()
        };
    }

    fn open_vib_menu(&mut self) {
        self.synth_pick_a = None;
        self.synth_vib_open = true;
        self.status_line = format!(
            "VIB {:.2} st · {:.1} Hz · {}",
            self.vibrato_depth,
            self.vibrato_rate,
            if self.vibrato_always > 0.01 {
                "ON"
            } else {
                "WHEEL"
            }
        );
        self.mark_dirty();
    }

    fn scroll_offset(&self, kind: ScrollKind) -> i32 {
        match kind {
            ScrollKind::SynthMorphPick => self.synth_pick_scroll,
            ScrollKind::KaossPicker => self.kaoss_picker_scroll,
            ScrollKind::KaossSettings => self.kaoss_settings_scroll,
            ScrollKind::SongList => self.song_scroll as i32,
            ScrollKind::Log => self.log_scroll as i32,
        }
    }

    fn set_scroll_offset(&mut self, kind: ScrollKind, value: i32) {
        match kind {
            ScrollKind::SynthMorphPick => {
                let grid = self.layout.synth_voice_grid(self.wave_names.len());
                self.synth_pick_scroll = grid.clamp_scroll(value);
            }
            ScrollKind::KaossPicker => {
                if let Some(kind) = self.kaoss_picker {
                    let n = crate::kaoss_ui::picker_count(kind, self.kaoss_show_all);
                    let grid = self.layout.kaoss_picker_grid(kind, n, self.kaoss_show_all);
                    self.kaoss_picker_scroll = grid.clamp_scroll(value);
                }
            }
            ScrollKind::KaossSettings => {
                self.kaoss_settings_scroll =
                    value.clamp(0, self.layout.kaoss_settings_max_scroll());
            }
            ScrollKind::SongList => {
                let list = self.layout.song_list_scroll(self.song_files.len());
                self.song_scroll = list.clamp_scroll(value.max(0) as usize);
            }
            ScrollKind::Log => {
                let list = self.layout.log_list_scroll(self.log_lines.len());
                self.log_scroll = list.clamp_scroll(value.max(0) as usize);
            }
        }
        self.mark_dirty();
    }

    fn apply_scroll_drag(
        &mut self,
        kind: ScrollKind,
        start_py: i32,
        py: i32,
        scroll_at_start: i32,
    ) {
        match kind {
            ScrollKind::SynthMorphPick | ScrollKind::KaossPicker | ScrollKind::KaossSettings => {
                let delta = start_py - py;
                self.set_scroll_offset(kind, scroll_at_start + delta);
            }
            ScrollKind::SongList => {
                let list = self.layout.song_list_scroll(self.song_files.len());
                let next = scroll::ListScroll::scroll_from_drag(
                    scroll_at_start as usize,
                    start_py,
                    py,
                    list.row_h,
                    list.max_scroll(),
                );
                if next != self.song_scroll {
                    self.song_scroll = next;
                    self.mark_dirty();
                }
            }
            ScrollKind::Log => {
                let list = self.layout.log_list_scroll(self.log_lines.len());
                let next = scroll::ListScroll::scroll_from_drag(
                    scroll_at_start as usize,
                    start_py,
                    py,
                    list.row_h,
                    list.max_scroll(),
                );
                if next != self.log_scroll {
                    self.log_scroll = next;
                    self.mark_dirty();
                }
            }
        }
    }

    fn resolve_scroll_tap(&mut self, kind: ScrollKind, px: i32, py: i32, outbox: &mut Outbox) {
        match kind {
            ScrollKind::SynthMorphPick => {
                let grid = self.layout.synth_voice_grid(self.wave_names.len());
                if let Some(index) = grid.index_at(px, py, self.synth_pick_scroll) {
                    self.assign_morph_endpoint(index, outbox);
                }
            }
            ScrollKind::KaossPicker => {
                if let Some(picker) = self.kaoss_picker {
                    let n = crate::kaoss_ui::picker_count(picker, self.kaoss_show_all);
                    let grid = self
                        .layout
                        .kaoss_picker_grid(picker, n, self.kaoss_show_all);
                    if let Some(index) = grid.index_at(px, py, self.kaoss_picker_scroll) {
                        self.apply_kaoss_picker(index, outbox);
                    }
                }
            }
            ScrollKind::SongList => {
                for row in 0..5 {
                    let cell = self.layout.song_row(row);
                    if cell.contains(px, py) {
                        let idx = self.song_scroll + row;
                        if idx < self.song_files.len() {
                            self.select_song(idx, outbox);
                        }
                        break;
                    }
                }
            }
            ScrollKind::Log => {}
            ScrollKind::KaossSettings => {}
        }
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
        let flanger_mix = self.fx_voice[3];
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
            flanger_mix,
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
        self.push_voice_fx("flanger_mix", self.fx_voice[3], outbox);
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
        self.push_voice_fx("flanger_mix", self.fx_voice[3], outbox);
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
            version: 2,
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
        self.vibrato_depth = 0.5;
        self.vibrato_rate = 5.0;
        self.morph_a = 0;
        self.morph_b = 1;
        outbox.morph_pair(self.morph_a, self.morph_b);
        outbox.synth("morph", self.synth_params[0]);
        outbox.synth("tone", self.synth_params[1]);
        outbox.synth("level", self.synth_params[2]);
        outbox.synth("attack", self.synth_params[3]);
        outbox.synth("release", self.synth_params[4]);
        self.push_vibrato_params(outbox);
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

    fn hit_chords_overlay(&self, px: i32, py: i32) -> Hit {
        if self.layout.chords_overlay_close().contains(px, py) {
            return Hit::ChordsOverlayClose;
        }
        match self.chords_overlay {
            Some(ChordsOverlay::Key) => {
                for pc in 0..12u8 {
                    if self
                        .layout
                        .chords_overlay_cell(pc as usize, 12)
                        .contains(px, py)
                    {
                        return Hit::ChordsKeyPick(pc);
                    }
                }
            }
            Some(ChordsOverlay::Changes) => {
                let n = chords::PROGRESSIONS.len();
                for i in 0..n {
                    if self.layout.chords_overlay_cell(i, n).contains(px, py) {
                        return Hit::ChordsChangesPick(i);
                    }
                }
            }
            None => {}
        }
        Hit::ChordsOverlayClose
    }

    fn chords_press_button(
        &mut self,
        gesture: u32,
        col: usize,
        row: QualityRow,
        outbox: &mut Outbox,
    ) {
        if let Some(slot) = self.chords_held.iter().position(|(g, _, _)| g.is_none()) {
            self.chords_held[slot] = (Some(gesture), col, row);
        }
        self.chords_sync_from_held(outbox);
    }

    fn chords_release_button(
        &mut self,
        gesture: u32,
        _col: usize,
        _row: QualityRow,
        outbox: &mut Outbox,
    ) {
        for slot in &mut self.chords_held {
            if slot.0 == Some(gesture) {
                *slot = (None, 0, QualityRow::Maj);
            }
        }
        if self.chords_hold {
            // Memory: keep last resolved chord sounding until a new one is chosen.
            return;
        }
        self.chords_sync_from_held(outbox);
        if !self.chords_contact_held() {
            self.chords_block_off(outbox);
        }
    }

    /// True while a grid button or palette pad finger is still down.
    fn chords_contact_held(&self) -> bool {
        self.chords_held.iter().any(|(g, _, _)| g.is_some())
            || self.fingers.iter().any(|f| {
                f.active
                    && matches!(
                        f.surface,
                        Surface::ChordsButton { .. } | Surface::ChordsPalette { .. }
                    )
            })
    }

    fn chords_held_list(&self) -> Vec<(usize, QualityRow)> {
        self.chords_held
            .iter()
            .filter_map(|(g, col, row)| g.map(|_| (*col, *row)))
            .collect()
    }

    /// Buttons currently held (for chord-grid highlight). Empty → use resolved chord.
    pub fn chords_held_buttons(&self) -> Vec<(usize, QualityRow)> {
        self.chords_held_list()
    }

    fn chords_sync_from_held(&mut self, outbox: &mut Outbox) {
        let held = self.chords_held_list();
        if let Some(spec) = chords::resolve_held(&held) {
            self.chords_select(spec, true, outbox);
        }
    }

    fn chords_select(&mut self, spec: ChordSpec, play_block: bool, outbox: &mut Outbox) {
        self.chords_current = Some(spec);
        self.status_line = spec.name();
        if play_block {
            self.chords_block_on(spec, outbox);
        }
    }

    fn chords_block_on(&mut self, spec: ChordSpec, outbox: &mut Outbox) {
        self.chords_block_off(outbox);
        let base = chords::block_base_for_octave(self.chords_octave);
        let notes = spec.block_notes_at(base);
        self.chords_block = notes;
        for note in notes.into_iter().flatten() {
            self.chords_note_on(note, 110, outbox);
        }
    }

    fn chords_block_off(&mut self, outbox: &mut Outbox) {
        for note in self.chords_block.into_iter().flatten() {
            self.chords_note_off(note, outbox);
        }
        self.chords_block = [None; 4];
    }

    fn nudge_chords_octave(&mut self, delta: i8, outbox: &mut Outbox) {
        let next = (self.chords_octave + delta).clamp(chords::OCTAVE_MIN, chords::OCTAVE_MAX);
        if next == self.chords_octave {
            return;
        }
        self.chords_octave = next;
        self.status_line = format!("chords {}", Self::c_note_label(chords::block_base_for_octave(next)));
        self.mark_dirty();
        if self.chords_block.iter().any(|n| n.is_some()) {
            if let Some(spec) = self.chords_current {
                self.chords_block_on(spec, outbox);
            }
        }
        if let Some(slot) = self
            .fingers
            .iter()
            .position(|f| f.active && matches!(f.surface, Surface::ChordsStrum))
        {
            let y = self.fingers[slot].y;
            self.chords_strum_note = None;
            self.chords_strum_to(y, outbox);
        }
    }

    fn chords_strum_to(&mut self, y: f32, outbox: &mut Outbox) {
        let Some(spec) = self.chords_current else {
            return;
        };
        let base = chords::strum_base_for_octave(self.chords_octave);
        let strings = spec.strum_strings_at(base);
        let note = chords::string_at(y, &strings);
        if self.chords_strum_note == Some(note) {
            return;
        }
        if let Some(prev) = self.chords_strum_note.take() {
            self.chords_note_off(prev, outbox);
        }
        self.chords_strum_note = Some(note);
        self.chords_note_on(note, 118, outbox);
    }

    fn chords_strum_off(&mut self, outbox: &mut Outbox) {
        if let Some(note) = self.chords_strum_note.take() {
            self.chords_note_off(note, outbox);
        }
    }

    fn chords_palette_tap(&mut self, slot: usize, outbox: &mut Outbox) {
        if slot >= PALETTE_SLOTS {
            return;
        }
        if self.chords_arm {
            self.chords_palette[slot] = self.chords_current;
            self.chords_arm = false;
            self.status_line = match self.chords_current {
                Some(c) => format!("palette {} ← {}", slot + 1, c.name()),
                None => format!("palette {} empty", slot + 1),
            };
            return;
        }
        if let Some(spec) = self.chords_palette[slot] {
            self.chords_select(spec, true, outbox);
        } else if let Some(spec) = self.chords_current {
            self.chords_palette[slot] = Some(spec);
            self.status_line = format!("palette {} ← {}", slot + 1, spec.name());
        }
    }

    fn chords_palette_press(&mut self, slot: usize, outbox: &mut Outbox) {
        if let Some(spec) = self.chords_palette.get(slot).copied().flatten() {
            self.chords_select(spec, true, outbox);
        }
    }

    fn chords_palette_release(&mut self, outbox: &mut Outbox) {
        if self.chords_hold {
            // Latch like the Omnichord memory / grid HOLD path.
            return;
        }
        if !self.chords_contact_held() {
            self.chords_block_off(outbox);
        }
    }

    fn load_changes(&mut self, index: usize) {
        let Some(prog) = chords::PROGRESSIONS.get(index) else {
            return;
        };
        self.chords_palette = chords::progression_in_key(prog, self.chords_key);
        self.chords_current = self.chords_palette.iter().copied().flatten().next();
        self.status_line = format!(
            "{} in {}",
            prog.name,
            chords::KEY_NAMES[self.chords_key as usize]
        );
        self.mark_dirty();
    }

    fn chords_note_on(&mut self, note: u8, vel: u8, outbox: &mut Outbox) {
        if self.chords_out.includes_local() {
            outbox.note_on(0, note, vel);
        }
        if self.chords_out.includes_usb() {
            outbox.midi_emit("note_on", 0, Some(note), Some(vel), None, None);
        }
        self.seq.push_note(true, 0, note, vel);
        self.push_pad_rec(true, 0, note, vel);
    }

    fn chords_note_off(&mut self, note: u8, outbox: &mut Outbox) {
        if self.chords_out.includes_local() {
            outbox.note_off(0, note);
        }
        if self.chords_out.includes_usb() {
            outbox.midi_emit("note_off", 0, Some(note), Some(0), None, None);
        }
        self.seq.push_note(false, 0, note, 0);
        self.push_pad_rec(false, 0, note, 0);
    }

    fn wipe_kaoss_fx(&mut self, outbox: &mut Outbox) {
        self.fx_bus = [0.0, 0.0, 0.0, 0.0];
        self.fx_voice[3] = 0.0;
        for name in Self::FX_PARAM_NAMES {
            outbox.fx_bus(name, 0.0);
        }
        self.push_voice_fx("flanger_mix", 0.0, outbox);
        let defaults = session::SessionState::default();
        self.synth_params[0] = defaults.morph;
        self.synth_params[1] = defaults.tone;
        outbox.synth("morph", defaults.morph);
        outbox.synth("tone", defaults.tone);
        outbox.synth("tone_lfo_amount", 0.0);
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
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("seq.mid")
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

    fn select_song(&mut self, idx: usize, outbox: &mut Outbox) {
        if idx >= self.song_files.len() {
            return;
        }
        self.song_selected = idx;
        let path = self.song_files[idx].clone();
        self.apply_song_file_tempo(&path, outbox);
        self.status_line = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("song")
            .to_string();
        self.mark_dirty();
    }

    fn apply_song_file_tempo(&mut self, path: &std::path::Path, outbox: &mut Outbox) {
        let Some(bpm) = songs::load_smf_bpm(path) else {
            return;
        };
        self.bpm = bpm.clamp(40.0, 240.0);
        self.seq.bpm = self.bpm;
        outbox.tempo(self.bpm);
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
        self.bpm = bpm.clamp(40.0, 240.0);
        self.seq.bpm = self.bpm;
        self.status_line = format!(
            "play {}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("song")
        );
    }

    fn paint_cells(&mut self) {
        let t = self.kaoss_viz_time;
        let mut finger_buf = [(0.0_f32, 0.0_f32); MAX_FINGERS];
        let n = self.copy_kaoss_fingers(&mut finger_buf);
        let fingers = &finger_buf[..n];
        let hold = self.kaoss_hold && self.kaoss_touching;
        let gate_flash = self.kaoss_gate_flash();
        let hue_shift = crate::kaoss_viz::program_hue(kaoss_ui::program(self.kaoss_program).id);
        let rainbow = crate::kaoss_viz::pad_color_is_rainbow(self.kaoss_mono_color);
        let solid = crate::kaoss_viz::pad_color_hs(self.kaoss_mono_color);
        for row in 0..LED_ROWS {
            for col in 0..LED_COLS {
                let excit = self.cell_amp[row][col];
                self.cells[row][col] = if rainbow {
                    crate::kaoss_viz::pad_led_rgb(
                        col,
                        row,
                        t,
                        fingers,
                        excit,
                        hold,
                        gate_flash,
                        hue_shift,
                    )
                } else {
                    let (h, s) = solid.unwrap_or((0.93, 0.88));
                    crate::kaoss_viz::pad_led_mono(
                        col, row, t, excit, hold, gate_flash, h, s,
                    )
                };
            }
        }
    }

    fn push_kaoss_trail(&mut self, x: f32, y: f32) {
        // Prefer updating the nearest recent spark so two fingers don't thrash
        // a single shared "last" point.
        const NEAR: f32 = 0.0004;
        let mut best: Option<usize> = None;
        let mut best_d2 = NEAR;
        for (i, p) in self.kaoss_trail.iter().enumerate() {
            let dx = p.0 - x;
            let dy = p.1 - y;
            let d2 = dx * dx + dy * dy;
            if d2 < best_d2 {
                best_d2 = d2;
                best = Some(i);
            }
        }
        if let Some(i) = best {
            self.kaoss_trail[i].0 = x;
            self.kaoss_trail[i].1 = y;
            self.kaoss_trail[i].2 = 1.0;
            return;
        }
        self.kaoss_trail.push((x, y, 1.0));
        if self.kaoss_trail.len() > 24 {
            let drop = self.kaoss_trail.len() - 24;
            self.kaoss_trail.drain(0..drop);
        }
    }

    fn age_kaoss_viz(&mut self, dt: f32) {
        self.kaoss_viz_time += dt;
        let trail_life = 0.45_f32;
        for p in &mut self.kaoss_trail {
            p.2 -= dt / trail_life;
        }
        self.kaoss_trail.retain(|p| p.2 > 0.0);
        // Expanding touch rings removed — they punched a black hole into the pad.
        self.kaoss_ripples.clear();
        if self.kaoss_viz_style.is_cells() {
            let mut finger_buf = [(0.0_f32, 0.0_f32); MAX_FINGERS];
            let n = self.copy_kaoss_fingers(&mut finger_buf);
            let fingers = &finger_buf[..n];
            let trail = self.kaoss_trail.clone();
            let mut cell_dt = dt;
            // First frame after a touch can be dt≈0 — still nudge attack forward.
            if n > 0 && cell_dt <= 0.0 {
                cell_dt = 0.02;
            }
            for row in 0..LED_ROWS {
                for col in 0..LED_COLS {
                    let target = crate::kaoss_viz::cell_excit_target(col, row, fingers, &trail);
                    self.cell_amp[row][col] =
                        crate::kaoss_viz::cell_step(self.cell_amp[row][col], target, cell_dt);
                }
            }
        }
        if self.kaoss_viz_style.is_glow() {
            let mut peak = 0.0_f32;
            for slot in 0..MAX_FINGERS {
                let touching = self.fingers[slot].active
                    && self.fingers[slot].surface == Surface::Kaoss;
                let target = if touching { 1.0 } else { 0.0 };
                let glow = &mut self.kaoss_glow[slot];
                let was_idle = glow.amp < 0.05;
                let mut glow_dt = dt;
                if target > glow.amp && glow_dt <= 0.0 {
                    glow_dt = 0.02;
                }
                glow.amp = crate::kaoss_viz::glow_step(glow.amp, target, glow_dt);
                if touching {
                    let x = self.fingers[slot].x;
                    let y = self.fingers[slot].y;
                    glow.xy = (x, y);
                    // Fresh contact: park shells under the fingertip (no leftover smear).
                    if was_idle {
                        glow.shells = [(x, y); crate::kaoss_viz::GLOW_LAG_COUNT];
                    } else {
                        for (i, shell) in glow.shells.iter_mut().enumerate() {
                            *shell = crate::kaoss_viz::glow_lag_step(
                                *shell,
                                (x, y),
                                glow_dt.max(dt),
                                crate::kaoss_viz::glow_lag_tau(i),
                            );
                        }
                    }
                } else if glow.amp > 0.01 {
                    // Release: shells ease toward last XY while amp fades.
                    let target_xy = glow.xy;
                    for (i, shell) in glow.shells.iter_mut().enumerate() {
                        *shell = crate::kaoss_viz::glow_lag_step(
                            *shell,
                            target_xy,
                            glow_dt.max(dt),
                            crate::kaoss_viz::glow_lag_tau(i),
                        );
                    }
                }
                peak = peak.max(glow.amp);
            }
            self.kaoss_glow_amp = peak;
        }
    }
}

fn downsample_wave(src: &[f32], dst: &mut [f32]) {
    let points = dst.len();
    if points == 0 || src.is_empty() {
        return;
    }
    let bucket = src.len() as f32 / points as f32;
    for i in 0..points {
        let a = ((i as f32) * bucket) as usize;
        let mut b = (((i + 1) as f32) * bucket) as usize;
        if b <= a {
            b = a + 1;
        }
        let b = b.min(src.len());
        let chunk = &src[a..b];
        let v = if (i & 1) == 0 {
            chunk.iter().copied().fold(f32::NEG_INFINITY, f32::max)
        } else {
            chunk.iter().copied().fold(f32::INFINITY, f32::min)
        };
        dst[i] = if v.is_finite() {
            v.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Outbox;
    use jambox_protocol::Request;

    #[test]
    fn kaoss_note_at_tracks_pad_x() {
        let model = NativeModel::new();
        let left = model.kaoss_note_at(0.05);
        let right = model.kaoss_note_at(0.95);
        assert_ne!(
            left, right,
            "left and right edges should map to different notes"
        );
        assert_eq!(
            kaoss_ui::midi_note_label(left),
            format!(
                "{}{}",
                jambox_core::NOTE_NAMES[(left % 12) as usize],
                (left as i32 / 12) - 1
            )
        );
    }

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
    fn kaoss_hold_survives_filter_pad_touch() {
        // HOLD a LEAD note, switch to FILTER, touch the pad — drone must stay.
        let mut model = NativeModel::new();
        model.kaoss_out = OutMode::Local;
        model.kaoss_hold = true;
        model.kaoss_program = kaoss_ui::KAOSS_PROGRAMS
            .iter()
            .position(|p| p.id == "lead")
            .expect("lead");
        let mut out = Outbox::new();
        let cell = model.layout.kaoss_cell(4, 3);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        let down = out.take();
        let hold_gesture = down.iter().find_map(|r| match r {
            Request::Touch {
                phase: TouchPhase::Down,
                gesture,
                ..
            } => Some(*gesture),
            _ => None,
        });
        assert!(hold_gesture.is_some(), "LEAD press should start a touch: {down:?}");
        model.finger_up(1, &mut out);
        let lift = out.take();
        assert!(
            lift.iter().all(|r| !matches!(
                r,
                Request::Touch {
                    phase: TouchPhase::Up,
                    ..
                }
            )),
            "HOLD lift must not note-off: {lift:?}"
        );
        assert!(model.kaoss_hold_gesture.is_some());

        model.kaoss_program = kaoss_ui::KAOSS_PROGRAMS
            .iter()
            .position(|p| p.id == "filter")
            .expect("filter");
        let filter_cell = model.layout.kaoss_cell(2, 2);
        model.finger_down(2, filter_cell.x + 4, filter_cell.y + 4, &mut out);
        let fx = out.take();
        assert!(
            fx.iter().all(|r| !matches!(
                r,
                Request::Touch {
                    phase: TouchPhase::Up,
                    ..
                }
            )),
            "FILTER press must not release the HOLD drone: {fx:?}"
        );
        assert_eq!(model.kaoss_hold_gesture, hold_gesture);
        assert!(
            fx.iter().any(|r| matches!(
                r,
                Request::Synth { param, .. } if param == "tone" || param == "morph"
            )) || fx.iter().any(|r| matches!(r, Request::Fx { .. })),
            "FILTER XY should still drive params: {fx:?}"
        );
    }

    #[test]
    fn kaoss_bend_program_emits_pitch_bend_semis() {
        let mut model = NativeModel::new();
        model.kaoss_program = kaoss_ui::KAOSS_PROGRAMS
            .iter()
            .position(|p| p.id == "bend")
            .expect("bend program");
        let mut out = Outbox::new();
        let k = model.layout.kaoss;
        // Touch top of pad → +12 semis.
        model.finger_down(1, k.x + k.w / 2, k.y + 4, &mut out);
        let batch = out.take();
        assert!(
            batch.iter().any(|r| matches!(
                r,
                Request::Synth { param, value, .. }
                    if param == "pitch_bend" && (*value - 12.0).abs() < 0.5
            )),
            "top of pad should bend +12 semis: {batch:?}"
        );
        // Midline → near 0.
        model.finger_move(1, k.x + k.w / 2, k.y + k.h / 2, &mut out);
        let batch = out.take();
        assert!(
            batch.iter().any(|r| matches!(
                r,
                Request::Synth { param, value, .. }
                    if param == "pitch_bend" && value.abs() < 0.5
            )),
            "midline should be near unison: {batch:?}"
        );
    }

    #[test]
    fn kaoss_y_move_emits_tone_for_lead() {
        let mut model = NativeModel::new();
        assert_eq!(kaoss_ui::program(model.kaoss_program).y_param, "tone");
        let mut out = Outbox::new();
        let cell = model.layout.kaoss_cell(6, 3);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        out.take();
        model.finger_move(1, cell.x + 4, cell.y + cell.h / 2, &mut out);
        let batch = out.take();
        assert!(
            batch.iter().any(|r| matches!(
                r,
                Request::Synth { param, .. } if param == "tone"
            )),
            "Y drag on LEAD should drive synth tone like Tk _emit_xy"
        );
    }

    #[test]
    fn kaoss_vib_y_drives_vibrato_not_tone() {
        let mut model = NativeModel::new();
        model.kaoss_program = kaoss_ui::KAOSS_PROGRAMS
            .iter()
            .position(|p| p.id == "vib")
            .expect("vib program");
        assert_eq!(kaoss_ui::program(model.kaoss_program).y_param, "vib");
        let tone_before = model.synth_params[1];
        let mut out = Outbox::new();
        let cell = model.layout.kaoss_cell(6, 1); // high Y → strong vib
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        let batch = out.take();
        assert!(
            batch.iter().any(|r| matches!(
                r,
                Request::Synth { param, .. } if param == "vibrato_depth" || param == "vibrato_always"
            )),
            "VIB should emit vibrato params: {batch:?}"
        );
        assert!(
            !batch.iter().any(|r| matches!(
                r,
                Request::Synth { param, .. } if param == "tone"
            )),
            "VIB must not scrub tone: {batch:?}"
        );
        assert!((model.synth_params[1] - tone_before).abs() < 1e-6);
        assert!(model.vibrato_always > 0.5);
    }

    #[test]
    fn kaoss_wah_y_drives_tone_lfo_rate_not_tone() {
        let mut model = NativeModel::new();
        model.kaoss_program = kaoss_ui::KAOSS_PROGRAMS
            .iter()
            .position(|p| p.id == "wah")
            .expect("wah program");
        assert_eq!(kaoss_ui::program(model.kaoss_program).y_param, "tone_lfo");
        let tone_before = model.synth_params[1];
        let mut out = Outbox::new();
        let cell = model.layout.kaoss_cell(6, 6); // top of pad → fast wah
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        let batch = out.take();
        assert!(
            batch.iter().any(|r| matches!(
                r,
                Request::Synth { param, value, .. }
                    if param == "tone_lfo_rate" && *value > 0.5
            )),
            "WAH Y should emit tone_lfo_rate: {batch:?}"
        );
        assert!(
            batch.iter().any(|r| matches!(
                r,
                Request::Synth { param, value, .. }
                    if param == "tone_lfo_amount" && *value > 0.5
            )),
            "WAH should arm the tone LFO: {batch:?}"
        );
        assert!(
            !batch.iter().any(|r| matches!(
                r,
                Request::Synth { param, .. } if param == "tone"
            )),
            "WAH must not scrub the sticky tone knob: {batch:?}"
        );
        assert!((model.synth_params[1] - tone_before).abs() < 1e-6);
        model.finger_up(1, &mut out);
        let lift = out.take();
        assert!(
            lift.iter().any(|r| matches!(
                r,
                Request::Synth { param, value, .. }
                    if param == "tone_lfo_amount" && *value < 0.01
            )),
            "lifting WAH should stop the tone LFO: {lift:?}"
        );
    }

    #[test]
    fn kaoss_wah_hold_keeps_tone_lfo_after_lift() {
        let mut model = NativeModel::new();
        model.kaoss_program = kaoss_ui::KAOSS_PROGRAMS
            .iter()
            .position(|p| p.id == "wah")
            .expect("wah program");
        let mut out = Outbox::new();
        model.toggle_kaoss_hold(&mut out);
        let cell = model.layout.kaoss_cell(4, 1); // low Y → slow wah
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        out.take();
        model.finger_up(1, &mut out);
        let lift = out.take();
        assert!(
            !lift.iter().any(|r| matches!(
                r,
                Request::Synth { param, value, .. }
                    if param == "tone_lfo_amount" && *value < 0.01
            )),
            "HOLD should keep the wah running after lift: {lift:?}"
        );
    }

    #[test]
    fn gate_multi_touch_survives_first_finger_up() {
        let mut model = NativeModel::new();
        model.kaoss_gate = 1; // GATE 1/8
        assert!(kaoss_ui::program(model.kaoss_program).note);
        assert!(kaoss_ui::gate(model.kaoss_gate).beats > 0.0);
        let mut out = Outbox::new();
        let a = model.layout.kaoss_cell(2, 3);
        let b = model.layout.kaoss_cell(9, 3);
        model.finger_down(1, a.x + 4, a.y + 4, &mut out);
        model.finger_down(2, b.x + 4, b.y + 4, &mut out);
        assert_eq!(model.active_fingers(), 2);
        assert!(model.kaoss_touching);
        out.take();
        model.finger_up(1, &mut out);
        assert_eq!(model.active_fingers(), 1);
        assert!(
            model.kaoss_touching,
            "lifting first gated finger must not kill the second"
        );
        // Force gate clock into the on phase and tick.
        model.kaoss_gate_t0 = Some(Instant::now());
        for _ in 0..45 {
            model.tick(1.0 / 60.0, &mut out);
        }
        let batch = out.take();
        assert!(
            batch.iter().any(|r| matches!(r, Request::Touch { .. })),
            "remaining finger should still receive gated touches: {batch:?}"
        );
    }

    #[test]
    fn kaoss_viz_tracks_multiple_fingers() {
        let mut model = NativeModel::new();
        model.kaoss_viz_style = crate::kaoss_viz::KaossVizStyle::Glow;
        let mut out = Outbox::new();
        let a = model.layout.kaoss_cell(1, 3);
        let b = model.layout.kaoss_cell(10, 3);
        model.finger_down(1, a.x + 4, a.y + 4, &mut out);
        model.finger_down(2, b.x + 4, b.y + 4, &mut out);
        for _ in 0..10 {
            model.tick(1.0 / 60.0, &mut out);
        }
        let lit: Vec<_> = model
            .kaoss_glow
            .iter()
            .enumerate()
            .filter(|(_, g)| g.amp > 0.2)
            .collect();
        assert!(
            lit.len() >= 2,
            "each Kaoss contact should drive its own glow bloom: {:?}",
            model.kaoss_glow.iter().map(|g| g.amp).collect::<Vec<_>>()
        );
        let dx = (lit[0].1.xy.0 - lit[1].1.xy.0).abs();
        assert!(dx > 0.4, "blooms should sit under different pad X positions");

        model.kaoss_viz_style = crate::kaoss_viz::KaossVizStyle::Cells;
        for _ in 0..10 {
            model.tick(1.0 / 60.0, &mut out);
        }
        let left = model.cell(1, 3);
        let right = model.cell(10, 3);
        let mid = model.cell(5, 3);
        let bright = |c: u32| ((c >> 16) & 0xff).max((c >> 8) & 0xff).max(c & 0xff);
        assert!(
            bright(left) > bright(mid) && bright(right) > bright(mid),
            "CELLS should light both fingers, not only the first"
        );
    }

    #[test]
    fn a_snare_can_fire_while_kick_repeats() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        model.drum_repeat[jambox_core::DrumModel::Kick.index()] = RepeatDivisionChoice::Quarter;
        let mut out = Outbox::new();
        let kick = model.layout.kit_pad_cell(4);
        let snare = model.layout.kit_pad_cell(5);
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
        assert!(batch
            .iter()
            .any(|r| matches!(r, Request::NoteOn { note: 37, .. })));
        assert!(!batch
            .iter()
            .any(|r| matches!(r, Request::Repeat { note: 37, .. })));
    }

    #[test]
    fn drum_edit_slider_emits_kit_macro() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        model.kit_edit_open = true;
        model.kit_selected = 4;
        let mut out = Outbox::new();
        let track = model.layout.kit_edit_slider(0);
        model.finger_down(1, track.x + 4, track.y + track.h / 4, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth {
                param,
                drum: Some(0),
                ..
            } if param == "drum_tone"
        )));
        assert!(model.edit_drum_macros()[0] > 0.5);
    }

    #[test]
    fn nav_changes_mode() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        assert_eq!(model.mode, UiMode::Kaoss);
        let cell = model.layout.nav_jam(3); // Pads
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
    fn kaoss_two_fingers_emit_two_touch_downs() {
        let mut model = NativeModel::new();
        assert!(kaoss_ui::program(model.kaoss_program).note);
        let mut out = Outbox::new();
        let a = model.layout.kaoss_cell(1, 3);
        let b = model.layout.kaoss_cell(10, 3);
        model.finger_down(1, a.x + 4, a.y + 4, &mut out);
        model.finger_down(2, b.x + 4, b.y + 4, &mut out);
        let batch = out.take();
        let downs: Vec<u32> = batch
            .iter()
            .filter_map(|r| match r {
                Request::Touch {
                    phase: TouchPhase::Down,
                    gesture,
                    ..
                } => Some(*gesture),
                _ => None,
            })
            .collect();
        assert_eq!(
            downs.len(),
            2,
            "each Kaoss contact must start its own engine voice: {batch:?}"
        );
        assert_ne!(downs[0], downs[1]);
        let mut buf = [(0.0, 0.0); MAX_FINGERS];
        assert_eq!(model.copy_kaoss_fingers(&mut buf), 2);
        assert!((buf[0].0 - buf[1].0).abs() > 0.4);
    }

    #[test]
    fn curated_program_pick_stores_absolute_index() {
        let mut model = NativeModel::new();
        model.kaoss_show_all = false;
        model.kaoss_picker = Some(KaossPicker::Program);
        let curated: Vec<_> = KAOSS_PROGRAMS
            .iter()
            .enumerate()
            .filter(|(_, p)| p.curated)
            .collect();
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
        assert!(batch
            .iter()
            .any(|r| matches!(r, Request::MorphPair { a: 2, b: 3 })));
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth { param, value, .. } if param == "morph" && (*value - 0.25).abs() < 1e-6
        )));
        assert!(batch
            .iter()
            .any(|r| matches!(r, Request::ClipLaunch { slot: 0, .. })));
    }

    #[test]
    fn fx_target_routes_to_voice() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Fx);
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
        model.set_mode(UiMode::Drums);
        model.kit_edit_open = true;
        model.ensure_library_loaded();
        let mut out = Outbox::new();
        let wave = model.layout.kit_wave;
        model.finger_down(1, wave.x + 4, wave.y + 4, &mut out);
        model.finger_up(1, &mut out);
        assert!(model.kit_edit_open);
        out.take();
        let track = model.layout.kit_edit_slider(0);
        model.finger_down(2, track.x + 4, track.y + track.h / 2, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth {
                param,
                drum: Some(0),
                ..
            } if param == "drum_tone"
        )));
    }

    #[test]
    fn kit_slider_on_one_drum_does_not_target_the_group() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        let mut out = Outbox::new();
        let snare = model.layout.kit_pad_cell(5);
        model.finger_down(1, snare.x + 4, snare.y + 4, &mut out);
        model.finger_up(1, &mut out);
        out.take();
        model.open_kit_edit();
        let track = model.layout.kit_edit_slider(2);
        model.finger_down(2, track.x + 4, track.y + 8, &mut out);
        let batch = out.take();
        let snare_model = jambox_core::drum_model_for_note(37).index() as u8;
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth {
                param,
                drum: Some(idx),
                ..
            } if param == "drum_pitch" && *idx == snare_model
        )));
        assert!(!batch.iter().any(|r| matches!(
            r,
            Request::Synth { param, drum: None, .. } if param == "drum_pitch"
        )));
    }

    #[test]
    fn all_drums_slider_writes_the_shared_kit_macros() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        let mut out = Outbox::new();
        let all = model.layout.kit_all;
        model.finger_down(1, all.x + 4, all.y + 4, &mut out);
        model.finger_up(1, &mut out);
        assert!(model.kit_all_drums);
        out.take();
        model.open_kit_edit();
        let track = model.layout.kit_edit_slider(0);
        model.finger_down(2, track.x + 4, track.y + 8, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth {
                param,
                drum: None,
                ..
            } if param == "drum_tone"
        )));
        let tone = model.drum_macros[0][0];
        assert!(model.drum_macros.iter().all(|m| (m[0] - tone).abs() < 1e-6));
    }

    #[test]
    fn holding_any_pad_starts_a_repeat_lane() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        let mut out = Outbox::new();
        let hat = model.layout.kit_pad_cell(0);
        model.finger_down(1, hat.x + 4, hat.y + 4, &mut out);
        model.finger_up(1, &mut out);
        out.take();
        let repeat_btn = model.layout.kit_note_repeat;
        model.finger_down(1, repeat_btn.x + 4, repeat_btn.y + 4, &mut out);
        model.finger_up(1, &mut out);
        assert!(model.kit_repeat_open);
        out.take();
        let eighth = model.layout.kit_repeat_choice_cell(2);
        model.finger_down(1, eighth.x + 4, eighth.y + 4, &mut out);
        model.finger_up(1, &mut out);
        out.take();
        model.finger_down(2, hat.x + 4, hat.y + 4, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Repeat {
                phase: RepeatPhase::Down,
                division: RepeatDivision::Eighth,
                ..
            }
        )));
    }

    #[test]
    fn note_repeat_off_sends_a_single_hit() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        let mut out = Outbox::new();
        let kick = model.layout.kit_pad_cell(4);
        model.finger_down(1, kick.x + 4, kick.y + 4, &mut out);
        model.finger_up(1, &mut out);
        let batch = out.take();
        assert!(batch
            .iter()
            .any(|r| matches!(r, Request::NoteOn { note: 36, .. })));
        assert!(batch
            .iter()
            .any(|r| matches!(r, Request::NoteOff { note: 36, .. })));
        assert!(!batch.iter().any(|r| matches!(r, Request::Repeat { .. })));
    }

    #[test]
    fn note_repeat_is_per_drum() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        let mut out = Outbox::new();
        let kick = model.layout.kit_pad_cell(4);
        let snare = model.layout.kit_pad_cell(5);
        model.finger_down(1, kick.x + 4, kick.y + 4, &mut out);
        model.finger_up(1, &mut out);
        out.take();
        let repeat_btn = model.layout.kit_note_repeat;
        model.finger_down(2, repeat_btn.x + 4, repeat_btn.y + 4, &mut out);
        model.finger_up(2, &mut out);
        let quarter = model.layout.kit_repeat_choice_cell(1);
        model.finger_down(2, quarter.x + 4, quarter.y + 4, &mut out);
        model.finger_up(2, &mut out);
        out.take();
        model.finger_down(3, snare.x + 4, snare.y + 4, &mut out);
        model.finger_up(3, &mut out);
        out.take();
        model.finger_down(3, repeat_btn.x + 4, repeat_btn.y + 4, &mut out);
        model.finger_up(3, &mut out);
        let none = model.layout.kit_repeat_choice_cell(0);
        model.finger_down(3, none.x + 4, none.y + 4, &mut out);
        model.finger_up(3, &mut out);
        out.take();
        model.finger_down(4, kick.x + 4, kick.y + 4, &mut out);
        model.finger_down(5, snare.x + 4, snare.y + 4, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Repeat {
                note: 36,
                division: RepeatDivision::Quarter,
                ..
            }
        )));
        assert!(batch
            .iter()
            .any(|r| matches!(r, Request::NoteOn { note: 37, .. })));
    }

    #[test]
    fn note_repeat_none_and_triple_are_in_the_picker() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        let mut out = Outbox::new();
        let kick = model.layout.kit_pad_cell(4);
        model.finger_down(1, kick.x + 4, kick.y + 4, &mut out);
        model.finger_up(1, &mut out);
        let repeat_btn = model.layout.kit_note_repeat;
        model.finger_down(2, repeat_btn.x + 4, repeat_btn.y + 4, &mut out);
        model.finger_up(2, &mut out);
        let triple = model.layout.kit_repeat_choice_cell(5);
        model.finger_down(2, triple.x + 4, triple.y + 4, &mut out);
        model.finger_up(2, &mut out);
        assert_eq!(
            model.drum_repeat[jambox_core::DrumModel::Kick.index()],
            RepeatDivisionChoice::Triple
        );
        out.take();
        model.finger_down(3, kick.x + 4, kick.y + 4, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Repeat {
                note: 36,
                division: RepeatDivision::QuarterTriplet,
                ..
            }
        )));
        assert_eq!(model.note_repeat_button_label(), "NOTE REPEAT: TRIPLE");
        model.finger_down(4, repeat_btn.x + 4, repeat_btn.y + 4, &mut out);
        model.finger_up(4, &mut out);
        let none = model.layout.kit_repeat_choice_cell(0);
        model.finger_down(4, none.x + 4, none.y + 4, &mut out);
        model.finger_up(4, &mut out);
        assert_eq!(
            model.drum_repeat[jambox_core::DrumModel::Kick.index()],
            RepeatDivisionChoice::Off
        );
        assert_eq!(model.note_repeat_button_label(), "NOTE REPEAT: OFF");
    }

    #[test]
    fn all_drums_note_repeat_writes_every_voice() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        let mut out = Outbox::new();
        let all = model.layout.kit_all;
        model.finger_down(1, all.x + 4, all.y + 4, &mut out);
        model.finger_up(1, &mut out);
        let repeat_btn = model.layout.kit_note_repeat;
        model.finger_down(2, repeat_btn.x + 4, repeat_btn.y + 4, &mut out);
        model.finger_up(2, &mut out);
        let sixteenth = model.layout.kit_repeat_choice_cell(4);
        model.finger_down(2, sixteenth.x + 4, sixteenth.y + 4, &mut out);
        model.finger_up(2, &mut out);
        assert!(model
            .drum_repeat
            .iter()
            .all(|d| *d == RepeatDivisionChoice::Sixteenth));
        assert_eq!(model.note_repeat_button_label(), "NOTE REPEAT: 1/16");
    }

    #[test]
    fn kit_scope_renders_a_one_shot() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        model.open_kit_edit();
        let mut out = Outbox::new();
        model.tick(1.0 / 60.0, &mut out);
        assert!(
            model.kit_wave.iter().any(|s| s.abs() > 0.01),
            "selected drum waveform should be non-silent"
        );
    }

    #[test]
    fn main_kit_view_does_not_rebuild_the_waveform_on_a_pad_tap() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Drums);
        model.kit_wave = [0.0; 160];
        model.kit_wave_dirty = false;
        let mut out = Outbox::new();
        let kick = model.layout.kit_pad_cell(4);
        model.finger_down(1, kick.x + 4, kick.y + 4, &mut out);
        model.finger_up(1, &mut out);
        model.tick(1.0 / 60.0, &mut out);
        assert!(
            model.kit_wave.iter().all(|s| *s == 0.0),
            "pad trigger must not rebuild the kit waveform"
        );
    }

    #[test]
    fn settings_update_does_not_block_the_ui() {
        host::set_dry_run(true);
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Settings);
        let mut out = Outbox::new();
        let btn = model.layout.settings_update;
        let start = Instant::now();
        model.finger_down(1, btn.x + 4, btn.y + 4, &mut out);
        model.finger_up(1, &mut out);
        assert!(
            start.elapsed() < std::time::Duration::from_millis(200),
            "UPDATE tap blocked for {:?}",
            start.elapsed()
        );
        assert_eq!(model.host_busy(), Some(HostTask::UpdateCheck));
        assert!(model.status_line.to_ascii_lowercase().contains("check"));
        let home = model.layout.nav_home();
        model.finger_down(2, home.x + 4, home.y + 4, &mut out);
        assert_eq!(model.mode, UiMode::Home);
        host::set_dry_run(false);
    }

    #[test]
    fn synth_octave_shifts_keyboard_notes() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Synth);
        let mut out = Outbox::new();
        let key = model.layout.synth_keyboard_white_rect(0); // C relative to C4
        model.finger_down(1, key.x + 4, key.y + key.h - 8, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::NoteOn {
                channel: 0,
                note: 60,
                velocity: 110
            }
        )));
        model.finger_up(1, &mut out);
        out.take();

        model.nudge_synth_octave(1);
        assert_eq!(model.synth_octave, 1);
        model.finger_down(2, key.x + 4, key.y + key.h - 8, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::NoteOn {
                channel: 0,
                note: 72,
                velocity: 110
            }
        )));
    }

    #[test]
    fn synth_lift_smear_does_not_kill_second_key() {
        // Finger A on C, finger B on E; A glides onto E then lifts — E must stay on.
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Synth);
        let mut out = Outbox::new();
        let c = model.layout.synth_keyboard_white_rect(0);
        let e = model.layout.synth_keyboard_white_rect(2);
        model.finger_down(1, c.x + 4, c.y + c.h - 8, &mut out);
        model.finger_down(2, e.x + 4, e.y + e.h - 8, &mut out);
        out.take();

        model.finger_move(1, e.x + 4, e.y + e.h - 8, &mut out);
        let glide = out.take();
        assert!(
            glide.iter().any(|r| matches!(r, Request::NoteOff { note: 60, .. })),
            "leaving C should note-off C: {glide:?}"
        );
        assert!(
            !glide
                .iter()
                .any(|r| matches!(r, Request::NoteOff { note: 64, .. })),
            "glide onto E must not note-off E while finger 2 holds it: {glide:?}"
        );
        assert!(
            !glide
                .iter()
                .any(|r| matches!(r, Request::NoteOn { note: 64, .. })),
            "E is already held — no second note-on: {glide:?}"
        );

        model.finger_up(1, &mut out);
        let lift = out.take();
        assert!(
            !lift
                .iter()
                .any(|r| matches!(r, Request::NoteOff { note: 64, .. })),
            "lifting the smeared finger must not kill E: {lift:?}"
        );

        model.finger_up(2, &mut out);
        let end = out.take();
        assert!(
            end.iter()
                .any(|r| matches!(r, Request::NoteOff { note: 64, .. })),
            "lifting the remaining finger should release E: {end:?}"
        );
    }

    #[test]
    fn chords_c_major_button_plays_local_triad() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Chords);
        model.chords_out = OutMode::Local;
        model.chords_hold = true;
        let mut out = Outbox::new();
        let c = crate::chords::col_for_root_pc(0);
        let cell = model.layout.chords_button(c, 0);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        let batch = out.take();
        let notes: Vec<u8> = batch
            .iter()
            .filter_map(|r| match r {
                Request::NoteOn {
                    channel: 0, note, ..
                } => Some(*note),
                _ => None,
            })
            .collect();
        assert!(notes.len() >= 3, "expected a triad, got {notes:?}");
        let pcs: Vec<u8> = notes.iter().map(|n| n % 12).collect();
        assert!(pcs.contains(&0) && pcs.contains(&4) && pcs.contains(&7));
        assert_eq!(model.chords_current.unwrap().name(), "C");
    }

    #[test]
    fn chords_octave_shifts_block_notes() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Chords);
        model.chords_out = OutMode::Local;
        model.chords_hold = true;
        model.chords_octave = 0;
        let mut out = Outbox::new();
        let c = crate::chords::col_for_root_pc(0);
        let cell = model.layout.chords_button(c, 0);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        let base_notes: Vec<u8> = out
            .take()
            .into_iter()
            .filter_map(|r| match r {
                Request::NoteOn { note, .. } => Some(note),
                _ => None,
            })
            .collect();
        assert!(!base_notes.is_empty());
        let oct = model.layout.chords_oct_up();
        model.finger_down(2, oct.x + 4, oct.y + 4, &mut out);
        model.finger_up(2, &mut out);
        let shifted: Vec<u8> = out
            .take()
            .into_iter()
            .filter_map(|r| match r {
                Request::NoteOn { note, .. } => Some(note),
                _ => None,
            })
            .collect();
        assert_eq!(model.chords_octave, 1);
        assert!(
            shifted.iter().any(|&n| base_notes.iter().any(|&b| n == b + 12)),
            "octave up should re-voice +12; base={base_notes:?} shifted={shifted:?}"
        );
    }

    #[test]
    fn chords_palette_mom_releases_on_lift() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Chords);
        model.chords_out = OutMode::Local;
        model.chords_hold = false;
        model.chords_palette[0] = Some(ChordSpec::new(0, chords::ChordQuality::Maj));
        let mut out = Outbox::new();
        let cell = model.layout.chords_palette_slot(0);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        let ons: Vec<_> = out
            .take()
            .into_iter()
            .filter(|r| matches!(r, Request::NoteOn { .. }))
            .collect();
        assert!(ons.len() >= 3, "palette press should sound, got {ons:?}");
        model.finger_up(1, &mut out);
        let offs: Vec<_> = out
            .take()
            .into_iter()
            .filter(|r| matches!(r, Request::NoteOff { .. }))
            .collect();
        assert!(
            offs.len() >= 3,
            "MOM palette lift should silence, got {offs:?}"
        );
        assert!(model.chords_block.iter().all(|n| n.is_none()));
    }

    #[test]
    fn chords_palette_hold_latches_after_lift() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Chords);
        model.chords_out = OutMode::Local;
        model.chords_hold = true;
        model.chords_palette[0] = Some(ChordSpec::new(0, chords::ChordQuality::Maj));
        let mut out = Outbox::new();
        let cell = model.layout.chords_palette_slot(0);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        out.take();
        model.finger_up(1, &mut out);
        let after = out.take();
        assert!(
            after.iter().all(|r| !matches!(r, Request::NoteOff { .. })),
            "HOLD should keep notes after lift, got {after:?}"
        );
        assert!(model.chords_block.iter().any(|n| n.is_some()));
    }

    #[test]
    fn chords_record_into_seq_backbone() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Chords);
        model.chords_out = OutMode::Local;
        model.chords_hold = false;
        assert!(matches!(
            model.seq.toggle_record(),
            crate::seq::SeqAction::Stop
        ));
        assert!(model.seq.is_recording());
        let mut out = Outbox::new();
        let c = crate::chords::col_for_root_pc(0);
        let cell = model.layout.chords_button(c, 0);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        let ons = model.seq.recorded_on_notes();
        assert!(
            ons.len() >= 3,
            "seq take should capture the triad, got {ons:?}"
        );
        let pcs: Vec<u8> = ons.iter().map(|n| n % 12).collect();
        assert!(pcs.contains(&0) && pcs.contains(&4) && pcs.contains(&7));
        model.finger_up(1, &mut out);
        std::thread::sleep(std::time::Duration::from_millis(20));
        match model.seq.toggle_record() {
            crate::seq::SeqAction::Upload { events, .. } => {
                assert!(events.iter().any(|e| e.on && e.note % 12 == 0));
            }
            _ => panic!("expected seq upload after chord take"),
        }
    }

    #[test]
    fn chords_record_into_pad_rec() {
        let mut model = NativeModel::new();
        model.pads_edit = true;
        model.pads_selected = 0;
        let mut out = Outbox::new();
        model.set_mode(UiMode::Pads);
        let rec = model.layout.pads_rec;
        model.finger_down(1, rec.x + 4, rec.y + 4, &mut out);
        model.finger_up(1, &mut out);
        assert!(model.pads_recording.is_some());
        model.set_mode(UiMode::Chords);
        model.chords_hold = false;
        let c = crate::chords::col_for_root_pc(0);
        let cell = model.layout.chords_button(c, 0);
        model.finger_down(2, cell.x + 4, cell.y + 4, &mut out);
        model.finger_up(2, &mut out);
        model.set_mode(UiMode::Pads);
        model.pads_edit = true;
        model.finger_down(3, rec.x + 4, rec.y + 4, &mut out);
        model.finger_up(3, &mut out);
        let ons: Vec<u8> = model.phrases[0]
            .events
            .iter()
            .filter(|e| e.on)
            .map(|e| e.note)
            .collect();
        assert!(
            ons.len() >= 3,
            "pad clip should capture the triad, got {ons:?}"
        );
        let pcs: Vec<u8> = ons.iter().map(|n| n % 12).collect();
        assert!(pcs.contains(&0) && pcs.contains(&4) && pcs.contains(&7));
    }

    #[test]
    fn chords_changes_fill_palette_in_key() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Chords);
        model.chords_key = 0;
        let pop = crate::chords::PROGRESSIONS
            .iter()
            .position(|p| p.id == "pop")
            .unwrap();
        model.load_changes(pop);
        let names: Vec<String> = model
            .chords_palette
            .iter()
            .flatten()
            .map(|c| c.name())
            .collect();
        assert_eq!(names, ["C", "G", "Am", "F"]);
    }

    #[test]
    fn settings_update_opens_panel_without_blocking_check() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Settings);
        let mut out = Outbox::new();
        let btn = model.layout.settings_update;
        model.finger_down(1, btn.x + 4, btn.y + 4, &mut out);
        model.finger_up(1, &mut out);
        assert!(model.update_panel_open, "UPDATE should open the submenu");
        assert!(!model.update_busy, "opening must not start a sync CHECK");
        assert!(model.update_job.is_none());
        assert!(model.update_status.contains("Running:"));
        assert!(model.update_status.contains("CHECK"));
        // CLOSE dismisses
        let close = model.layout.update_close;
        model.finger_down(2, close.x + 4, close.y + 4, &mut out);
        model.finger_up(2, &mut out);
        assert!(!model.update_panel_open);
    }

    #[test]
    fn synth_flange_slider_sends_voice_flanger_mix() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Synth);
        model.morph_a = 2;
        model.morph_b = 5;
        let mut out = Outbox::new();
        let track = model.layout.synth_slider(5);
        model.finger_down(1, track.x + 4, track.y + 8, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Fx {
                target: jambox_protocol::FxTargetSpec::Voice { index: 2 },
                param,
                ..
            } if param == "flanger_mix"
        )));
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Fx {
                target: jambox_protocol::FxTargetSpec::Voice { index: 5 },
                param,
                ..
            } if param == "flanger_mix"
        )));
        assert!(model.fx_voice[3] > 0.5);
    }

    #[test]
    fn fx_menu_bus_flange_sends_global_flanger_mix() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Fx);
        model.fx_target = FxEditTarget::Bus;
        let mut out = Outbox::new();
        let track = model.layout.settings_fx_slider(3);
        // Top of the slider → wet ≈ 1.
        model.finger_down(1, track.x + 8, track.y + 4, &mut out);
        let batch = out.take();
        assert!(
            batch.iter().any(|r| matches!(
                r,
                Request::Fx {
                    target: jambox_protocol::FxTargetSpec::Bus,
                    param,
                    ..
                } if param == "flanger_mix"
            )),
            "expected bus flanger_mix, got {batch:?}"
        );
        assert!(model.fx_bus[3] > 0.5);
    }

    #[test]
    fn settings_no_longer_hosts_fx_sliders() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Settings);
        let mut out = Outbox::new();
        let track = model.layout.settings_fx_slider(0);
        model.finger_down(1, track.x + 8, track.y + 8, &mut out);
        let batch = out.take();
        assert!(
            batch.iter().all(|r| !matches!(r, Request::Fx { .. })),
            "Settings must not own FX sliders: {batch:?}"
        );
    }

    #[test]
    fn wifi_settings_opens_panel() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Settings);
        // Avoid auto-scan thread in the unit test.
        model.wifi_networks.push(crate::wifi::WifiNetwork {
            ssid: "TestNet".into(),
            signal: 70,
            security: "WPA2".into(),
            in_use: false,
        });
        let mut out = Outbox::new();
        let btn = model.layout.settings_wifi;
        model.finger_down(1, btn.x + 4, btn.y + 4, &mut out);
        model.finger_up(1, &mut out);
        assert!(model.wifi_panel_open);
        let row = model.layout.wifi_row(0);
        model.finger_down(2, row.x + 4, row.y + 4, &mut out);
        model.finger_up(2, &mut out);
        assert!(model.wifi_kb_open, "secured net should open password keyboard");
        assert_eq!(model.wifi_kb_ssid, "TestNet");
    }

    #[test]
    fn fm_mode_enables_engine_and_plays_a_key() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let tile = model.layout.home_tile(1);
        model.set_mode(UiMode::Home);
        model.finger_down(1, tile.x + 8, tile.y + 8, &mut out);
        model.finger_up(1, &mut out);
        assert_eq!(model.mode, UiMode::Fm);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth { param, value, .. } if param == "fm_enable" && *value > 0.5
        )));
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::KnobMap { mode, .. } if mode == "fm"
        )));

        let rec = model.layout.fm_recipe_cell(1);
        model.finger_down(2, rec.x + 4, rec.y + 4, &mut out);
        model.finger_up(2, &mut out);
        assert_eq!(model.fm_recipe, 1);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth { param, value, .. } if param == "fm_recipe" && (*value - 1.0).abs() < 0.1
        )));

        let key = model.layout.synth_keyboard_white_rect(0);
        model.finger_down(3, key.x + 4, key.y + 4, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(r, Request::NoteOn { .. })));
        model.finger_up(3, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(r, Request::NoteOff { .. })));
    }

    #[test]
    fn drawing_one_operator_into_another_sends_a_link() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        model.switch_mode(UiMode::Fm, &mut out);
        out.take();
        let (ax, ay) = model.layout.fm_op_center(0);
        let (dx, dy) = model.layout.fm_op_center(3);
        model.finger_down(1, ax, ay, &mut out);
        model.finger_move(1, dx, dy, &mut out);
        model.finger_up(1, &mut out);
        let batch = out.take();
        let packed = jambox_core::pack_fm_link(0, 3, 0.7);
        assert!(
            batch.iter().any(|r| matches!(
                r,
                Request::Synth { param, value, .. }
                    if param == "fm_connect" && (*value - packed).abs() < 0.05
            )),
            "{batch:?}"
        );
        assert!((model.fm_matrix[0][3] - 0.7).abs() < 0.05);
        assert_eq!(model.fm_selected, 0);

        let clear = model.layout.fm_clear();
        model.finger_down(2, clear.x + 4, clear.y + 4, &mut out);
        model.finger_up(2, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth { param, .. } if param == "fm_clear"
        )));
        assert_eq!(model.fm_matrix[0][3], 0.0);
    }

    #[test]
    fn tapping_an_operator_selects_it() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        model.switch_mode(UiMode::Fm, &mut out);
        out.take();
        let (bx, by) = model.layout.fm_op_center(1);
        model.finger_down(1, bx, by, &mut out);
        model.finger_up(1, &mut out);
        assert_eq!(model.fm_selected, 1);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth { param, value, .. } if param == "fm_op" && (*value - 1.0).abs() < 0.1
        )));
    }

    #[test]
    fn leaving_fm_disables_the_playground() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        model.switch_mode(UiMode::Fm, &mut out);
        out.take();
        model.switch_mode(UiMode::Synth, &mut out);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Synth { param, value, .. } if param == "fm_enable" && *value < 0.5
        )));
    }
}
