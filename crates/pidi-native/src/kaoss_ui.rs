//! Kaoss program / picker / gate helpers (parity with `apps/pidi/pidi/kaoss.py`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KaossPicker {
    Program,
    Scale,
    Key,
    Octave,
    Gate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KaossProgram {
    pub id: &'static str,
    pub label: &'static str,
    /// `true` = finger starts a scale note. `false` = sculpts an already-playing sound.
    pub note: bool,
    pub y_param: &'static str,
    pub x_param: Option<&'static str>,
    pub curated: bool,
}

/// Job family for a program (derived from `note` + `midi_gesture()`).
///
/// - **Note** — pad pitch on X (`note: true`)
/// - **Fx** — local synth / insert / punch params (`note: false`, not MIDI-out)
/// - **Midi** — external CC / channel-pressure gestures (`midi_gesture()`)
///
/// WIDTH / SWEEP are **Fx**: they drive local `synth("width"|"sweep")`, not USB CC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KaossProgramCategory {
    Note,
    Fx,
    Midi,
}

/// PROG picker filter chips. Default **Note** (Kaossilator-first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KaossProgramFilter {
    Note,
    Fx,
    Midi,
    All,
}

impl Default for KaossProgramFilter {
    fn default() -> Self {
        Self::Note
    }
}

impl KaossProgramFilter {
    pub const ALL: [Self; 4] = [Self::Note, Self::Fx, Self::Midi, Self::All];

    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "NOTE",
            Self::Fx => "FX",
            Self::Midi => "MIDI",
            Self::All => "ALL",
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Fx => "fx",
            Self::Midi => "midi",
            Self::All => "all",
        }
    }

    pub fn from_wire(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "fx" => Self::Fx,
            "midi" => Self::Midi,
            "all" => Self::All,
            _ => Self::Note,
        }
    }

    pub fn matches(self, cat: KaossProgramCategory) -> bool {
        match self {
            Self::All => true,
            Self::Note => cat == KaossProgramCategory::Note,
            Self::Fx => cat == KaossProgramCategory::Fx,
            Self::Midi => cat == KaossProgramCategory::Midi,
        }
    }

    /// Chip fill when selected.
    pub fn selected_color(self) -> u32 {
        match self {
            Self::Note => 0x458588,
            Self::Fx => 0xd65d0e,
            Self::Midi => 0xb16286,
            Self::All => 0x689d6a,
        }
    }
}

impl KaossProgram {
    /// Derived category for filter chips / chrome tags.
    pub fn category(self) -> KaossProgramCategory {
        if self.note {
            KaossProgramCategory::Note
        } else if self.midi_gesture().is_some() {
            KaossProgramCategory::Midi
        } else {
            KaossProgramCategory::Fx
        }
    }

    /// Short family tag for chrome / picker.
    pub fn family_tag(self) -> &'static str {
        match self.category() {
            KaossProgramCategory::Note => "NOTE",
            KaossProgramCategory::Fx => "FX",
            KaossProgramCategory::Midi => "MIDI",
        }
    }

    /// Idle tile fill in the PROG picker (selected uses a brighter cousin).
    pub fn picker_color(self) -> u32 {
        match self.category() {
            KaossProgramCategory::Note => 0x458588, // teal — plays a tone
            KaossProgramCategory::Fx => 0xd65d0e,   // orange — local scrub
            KaossProgramCategory::Midi => 0xb16286, // purple — USB gestures
        }
    }

    pub fn picker_selected_color(self) -> u32 {
        match self.category() {
            KaossProgramCategory::Note => 0x689d6a,
            KaossProgramCategory::Fx => 0xfe8019,
            KaossProgramCategory::Midi => 0xd3869b,
        }
    }

    /// Dimmed idle fill (e.g. MIDI tiles when KAOSS out is Local-only).
    pub fn picker_dim_color(self) -> u32 {
        match self.category() {
            KaossProgramCategory::Note => 0x2a4a4c,
            KaossProgramCategory::Fx => 0x6a3a18,
            KaossProgramCategory::Midi => 0x5a3450,
        }
    }

    /// PROG toolbar button fill.
    pub fn chrome_color(self) -> u32 {
        self.picker_color()
    }

    /// True when pad X and/or Y drives the left/right sweep (pan).
    pub fn uses_sweep(self) -> bool {
        self.y_param == "sweep" || self.x_param == Some("sweep")
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GatePattern {
    pub id: &'static str,
    pub label: &'static str,
    pub beats: f64,
    pub duty: f64,
}

/// Full program table — curated first, then the rest (SHOW ALL).
/// Indices 0..=20 are stable (sessions persist `kaoss_program` as an index);
/// FX twins append after that.
pub const KAOSS_PROGRAMS: &[KaossProgram] = &[
    KaossProgram {
        id: "lead",
        label: "LEAD",
        note: true,
        y_param: "tone",
        x_param: None,
        curated: true,
    },
    KaossProgram {
        id: "morph",
        label: "MORPH",
        note: true,
        y_param: "morph",
        x_param: None,
        curated: true,
    },
    KaossProgram {
        id: "vib",
        label: "VIB",
        note: true,
        y_param: "vib",
        x_param: None,
        curated: true,
    },
    KaossProgram {
        id: "filter",
        label: "FILTER",
        note: false,
        y_param: "morph",
        x_param: Some("tone"),
        curated: true,
    },
    // X = BPM-synced delay time (1/4..1/32); Y = mix. See `tempo_fx`.
    KaossProgram {
        id: "echo",
        label: "ECHO",
        note: false,
        y_param: "delay_mix",
        x_param: Some("delay_time"),
        curated: true,
    },
    KaossProgram {
        id: "drive",
        label: "DRIVE",
        note: false,
        y_param: "reverb_mix",
        x_param: Some("drive"),
        curated: true,
    },
    // X = scale pitch (plays); Y = flange amount.
    KaossProgram {
        id: "flange",
        label: "FLANGE",
        note: true,
        y_param: "flanger_mix",
        x_param: None,
        curated: true,
    },
    KaossProgram {
        id: "level",
        label: "LEVEL",
        note: true,
        y_param: "level",
        x_param: None,
        curated: false,
    },
    KaossProgram {
        id: "decay",
        label: "DECAY",
        note: true,
        y_param: "release",
        x_param: None,
        curated: false,
    },
    KaossProgram {
        id: "attack",
        label: "ATTACK",
        note: true,
        y_param: "attack",
        x_param: None,
        curated: false,
    },
    KaossProgram {
        id: "grit",
        label: "GRIT",
        note: true,
        y_param: "drive",
        x_param: None,
        curated: false,
    },
    KaossProgram {
        id: "delay",
        label: "DELAY",
        note: true,
        y_param: "delay_mix",
        x_param: None,
        curated: false,
    },
    KaossProgram {
        id: "swell",
        label: "SWELL",
        note: false,
        y_param: "delay_mix",
        x_param: Some("attack"),
        curated: false,
    },
    KaossProgram {
        id: "space",
        label: "SPACE",
        note: false,
        y_param: "reverb_mix",
        x_param: Some("delay_mix"),
        curated: false,
    },
    KaossProgram {
        id: "reso",
        label: "RESO",
        note: false,
        y_param: "delay_fb",
        x_param: Some("tone"),
        curated: false,
    },
    KaossProgram {
        id: "wash",
        label: "WASH",
        note: false,
        y_param: "reverb_mix",
        x_param: Some("reverb_size"),
        curated: false,
    },
    KaossProgram {
        id: "crush",
        label: "CRUSH",
        note: false,
        y_param: "tone",
        x_param: Some("drive"),
        curated: false,
    },
    // X = tone; Y = BPM-synced delay time (1/4..1/32). Mix-desk overlay.
    KaossProgram {
        id: "dub",
        label: "DUB",
        note: false,
        y_param: "delay_time",
        x_param: Some("tone"),
        curated: false,
    },
    // X = scale pitch; Y = pitch bend about the pad midline (center = 0).
    KaossProgram {
        id: "bend",
        label: "BEND",
        note: true,
        y_param: "pitch_bend",
        x_param: None,
        curated: true,
    },
    // X = scale pitch; Y = BPM-synced auto-wah rate (1 cycle per 1/4..1/32).
    KaossProgram {
        id: "wah",
        label: "WAH",
        note: true,
        y_param: "tone_lfo",
        x_param: None,
        curated: true,
    },
    // FX twin of FLANGE — BPM-synced rate (X) + mix (Y), no pad note.
    KaossProgram {
        id: "flange_rate",
        label: "FL RATE",
        note: false,
        y_param: "flanger_mix",
        x_param: Some("flanger_rate"),
        curated: true,
    },
    // FX twins appended so existing session program indices stay stable.
    // X = vibrato rate (1–9 Hz); Y = depth. No pad note.
    KaossProgram {
        id: "vib_fx",
        label: "VIB FX",
        note: false,
        y_param: "vib",
        x_param: Some("vibrato_rate"),
        curated: true,
    },
    KaossProgram {
        id: "bend_fx",
        label: "BEND FX",
        note: false,
        y_param: "pitch_bend",
        x_param: None,
        curated: true,
    },
    // Same BPM-synced tone LFO as WAH, without starting a pad note.
    KaossProgram {
        id: "wah_fx",
        label: "WAH FX",
        note: false,
        y_param: "tone_lfo",
        x_param: None,
        curated: true,
    },
    KaossProgram {
        id: "level_fx",
        label: "LVL FX",
        note: false,
        y_param: "level",
        x_param: None,
        curated: false,
    },
    KaossProgram {
        id: "decay_fx",
        label: "DEC FX",
        note: false,
        y_param: "release",
        x_param: None,
        curated: false,
    },
    KaossProgram {
        id: "attack_fx",
        label: "ATK FX",
        note: false,
        y_param: "attack",
        x_param: None,
        curated: false,
    },
    // MIDI gestures for a sound that is already playing. Appended so older
    // session program indices stay put.
    KaossProgram {
        id: "expr",
        label: "EXPR",
        note: false,
        y_param: "expr",
        x_param: None,
        curated: true,
    },
    KaossProgram {
        id: "bright",
        label: "BRIGHT",
        note: false,
        y_param: "bright",
        x_param: Some("reso"),
        curated: true,
    },
    KaossProgram {
        id: "press",
        label: "PRESS",
        note: false,
        y_param: "pressure",
        x_param: None,
        curated: true,
    },
    KaossProgram {
        id: "glide",
        label: "GLIDE",
        note: false,
        y_param: "glide",
        x_param: None,
        curated: true,
    },
    KaossProgram {
        id: "pan",
        label: "PAN",
        note: false,
        y_param: "pan_y",
        x_param: Some("pan"),
        curated: true,
    },
    KaossProgram {
        id: "mod",
        label: "MOD",
        note: false,
        y_param: "mod",
        x_param: None,
        curated: true,
    },
    // Local stereo pan. Appended after the MIDI gestures so those indices stay put.
    // X = left/right. Y is the same param so the program is sweep-only; apply uses X.
    KaossProgram {
        id: "sweep",
        label: "SWEEP",
        note: false,
        y_param: "sweep",
        x_param: Some("sweep"),
        curated: true,
    },
    KaossProgram {
        id: "width",
        label: "WIDTH",
        note: false,
        y_param: "width",
        x_param: None,
        curated: true,
    },
    // SPIKE 4 — signature destroy gestures. Ride Punch DSP under the hood
    // without cloning the Punch rack into PROG. Appended so older session
    // program indices stay put. (Punch SLICE stays on the punch-link strip —
    // a dedicated PROG was redundant.)
    KaossProgram {
        id: "scream",
        label: "SCREAM",
        note: false,
        y_param: "drive",
        x_param: Some("isolator"),
        curated: true,
    },
    // X = tape/motor slowdown; Y = crush/noise. Replaces retired SLICE PROG
    // at the same index so saved favs / kaoss_program pointing at old SLICE
    // land on VINYL instead of shifting SCREAM.
    KaossProgram {
        id: "vinyl",
        label: "VINYL",
        note: false,
        y_param: "punch_crush",
        x_param: Some("punch_tape"),
        curated: true,
    },
    KaossProgram {
        id: "drop",
        label: "DROP",
        note: false,
        y_param: "isolator",
        x_param: Some("punch_drop"),
        curated: true,
    },
];

/// Instant PROG recall slots (favorites). `KAOSS_FAV_EMPTY` = unassigned.
pub const KAOSS_FAV_SLOTS: usize = 6;
pub const KAOSS_FAV_EMPTY: u16 = u16::MAX;

/// Channel-wide MIDI a Kaoss FX program writes while a note is already sounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KaossMidiGesture {
    /// CC 11. Rest is full (127) so a lift does not leave the note quiet.
    Expression,
    /// CC 74 brightness, CC 71 resonance. Stays where you leave it.
    Brightness,
    /// Channel pressure. Rest is 0.
    Pressure,
    /// CC 65 on while touching, CC 5 time from Y. Off on lift.
    Glide,
    /// CC 10 from X. Center of the pad is center pan.
    Pan,
    /// CC 1. The wheel itself, not a pitch LFO. Stays where you leave it.
    ModWheel,
}

impl KaossProgram {
    pub fn midi_gesture(self) -> Option<KaossMidiGesture> {
        match self.id {
            "expr" => Some(KaossMidiGesture::Expression),
            "bright" => Some(KaossMidiGesture::Brightness),
            "press" => Some(KaossMidiGesture::Pressure),
            "glide" => Some(KaossMidiGesture::Glide),
            "pan" => Some(KaossMidiGesture::Pan),
            "mod" => Some(KaossMidiGesture::ModWheel),
            _ => None,
        }
    }

    /// True when this axis name is driven by pad X and/or Y.
    pub fn drives_param(self, name: &str) -> bool {
        self.y_param == name || self.x_param == Some(name)
    }

    /// SPIKE 1 — Kaoss-Pad wet engage.
    ///
    /// FX-class programs (`note == false`) that sculpt delay / reverb / flange
    /// need an audible wet mix while the finger is down. When XY already owns
    /// `*_mix`, the finger position *is* the engage. When XY only owns time /
    /// feedback / size / rate (DUB, RESO, …), the matching mix must be bumped
    /// on touch so the pad is not a dry no-op.
    ///
    /// NOTE programs return empty — they play a tone; optional wet on Y is
    /// already applied by `apply_kaoss_xy`.
    pub fn wet_engage_mixes(self) -> &'static [&'static str] {
        if self.note || self.midi_gesture().is_some() {
            return &[];
        }
        let delay = self.drives_param("delay_mix")
            || self.drives_param("delay_time")
            || self.drives_param("delay_fb");
        let reverb = self.drives_param("reverb_mix") || self.drives_param("reverb_size");
        let flange = self.drives_param("flanger_mix")
            || self.drives_param("flanger_rate")
            || self.drives_param("flanger_depth")
            || self.drives_param("flanger_fb");
        // Static tables keep this allocation-free on the UI hot path.
        match (
            delay && !self.drives_param("delay_mix"),
            reverb && !self.drives_param("reverb_mix"),
            flange && !self.drives_param("flanger_mix"),
        ) {
            (true, true, true) => &["delay_mix", "reverb_mix", "flanger_mix"],
            (true, true, false) => &["delay_mix", "reverb_mix"],
            (true, false, true) => &["delay_mix", "flanger_mix"],
            (false, true, true) => &["reverb_mix", "flanger_mix"],
            (true, false, false) => &["delay_mix"],
            (false, true, false) => &["reverb_mix"],
            (false, false, true) => &["flanger_mix"],
            (false, false, false) => &[],
        }
    }

    /// True when pad XY drives Punch rack slots (destroy gestures).
    pub fn uses_punch_destroy(self) -> bool {
        self.drives_param("punch_slice")
            || self.drives_param("punch_crush")
            || self.drives_param("punch_rpt")
            || self.drives_param("punch_tape")
            || self.drives_param("punch_drop")
            || self.drives_param("isolator")
    }
}

/// Default instant PROG slots: playable + destroy pair + a couple staples.
pub fn default_prog_favs() -> [u16; KAOSS_FAV_SLOTS] {
    let mut out = [KAOSS_FAV_EMPTY; KAOSS_FAV_SLOTS];
    for (i, id) in ["lead", "filter", "echo", "scream", "vinyl", "flange"]
        .iter()
        .enumerate()
    {
        if let Some(idx) = KAOSS_PROGRAMS.iter().position(|p| p.id == *id) {
            out[i] = idx as u16;
        }
    }
    out
}

pub fn fav_label(slot: usize, favs: &[u16; KAOSS_FAV_SLOTS]) -> String {
    let slot = slot % KAOSS_FAV_SLOTS;
    match favs[slot] {
        KAOSS_FAV_EMPTY => format!("F{}", slot + 1),
        idx => {
            let prog = program(idx as usize);
            // Short chrome: F3 SCR… — full label on status.
            let tag = if prog.label.len() <= 5 {
                prog.label.to_string()
            } else {
                format!("{}…", &prog.label[..4])
            };
            format!("F{} {tag}", slot + 1)
        }
    }
}

/// SPIKE 1 — default wet amount when touch engages a mix XY does not own.
/// High enough to hear clearly over a playing voice / kit; panel values above
/// this are left alone (`max(panel, ENGAGE)`).
pub const KAOSS_WET_ENGAGE: f32 = 0.55;

/// SPIKE 1 — soft FX Release length (seconds). Lift ramps wet mixes toward the
/// pre-touch spring instead of hard-cutting, so delay/reverb tanks stay
/// audible briefly (existing `FxUnit` tails only speak while mix > 0).
pub const KAOSS_FX_RELEASE_SEC: f32 = 0.22;

/// Full-pad Y travel maps to ± this many semitones (center Y = 0).
pub const PITCH_BEND_RANGE_SEMIS: f32 = jambox_core::PITCH_BEND_RANGE_SEMIS;

/// Half-width of the BEND rest band, in pad-Y units (0..1).
/// `y` in `0.5 ± this` is unison so a finger can sit on the midline.
pub const BEND_CENTER_DEADZONE: f32 = jambox_core::BEND_CENTER_DEADZONE;

/// Bottom rest band for VIB (and similar 0-at-rest Y maps), in pad-Y units.
pub const ZERO_REST_DEADZONE: f32 = 0.08;

/// Half-width of the SWEEP rest band around the pad midline (center = both).
pub const SWEEP_CENTER_DEADZONE: f32 = 0.08;

/// Collapse a center-zero axis through a dead band, then remap so the
/// remaining travel still reaches 0 and 1 (no step at the dead-zone edge).
pub fn apply_center_deadzone(y: f32, dead: f32) -> f32 {
    jambox_core::apply_center_deadzone(y, dead)
}

/// Collapse a 0-at-rest axis through a bottom dead band, then remap 0..1.
pub fn apply_zero_deadzone(y: f32, dead: f32) -> f32 {
    let y = y.clamp(0.0, 1.0);
    let dead = dead.clamp(0.0, 0.95);
    if y <= dead {
        return 0.0;
    }
    (y - dead) / (1.0 - dead)
}

/// Pad Y (0 = bottom, 1 = top) → pitch-bend semitones. Midline is unison.
pub fn y_to_pitch_bend_semis(y: f32) -> f32 {
    jambox_core::y_to_bend_semis(y)
}

/// MIDI 14-bit pitch wheel (8192 = center) for a bend in semitones.
pub fn semis_to_pitch_bend_midi(semis: f32) -> u16 {
    let t = (semis / PITCH_BEND_RANGE_SEMIS).clamp(-1.0, 1.0);
    (8192.0 + t * 8192.0).round().clamp(0.0, 16383.0) as u16
}

/// MIDI 14-bit pitch wheel (8192 = center) for a pad-Y bend.
pub fn y_to_pitch_bend_midi(y: f32) -> u16 {
    semis_to_pitch_bend_midi(y_to_pitch_bend_semis(y))
}

/// Gate set: off / 1/4 / 1/8 / 1/16 / trip (1/4 fills the gap vs drum divisions).
pub const GATE_PATTERNS: &[GatePattern] = &[
    GatePattern {
        id: "off",
        label: "GATE OFF",
        beats: 0.0,
        duty: 0.0,
    },
    GatePattern {
        id: "4th",
        label: "GATE 1/4",
        beats: 1.0,
        duty: 0.55,
    },
    GatePattern {
        id: "8th",
        label: "GATE 1/8",
        beats: 0.5,
        duty: 0.55,
    },
    GatePattern {
        id: "16th",
        label: "GATE 1/16",
        beats: 0.25,
        duty: 0.50,
    },
    GatePattern {
        id: "trip",
        label: "GATE TRIP",
        beats: 1.0 / 3.0,
        duty: 0.50,
    },
];

pub const OCTAVE_LABELS: [&str; 4] = ["1 OCT", "2 OCT", "3 OCT", "4 OCT"];
/// Left-edge C of the pad. C1 .. C8, paired with a 1..4 octave width.
pub const ROOT_OCTAVE_MIDI: [u8; 8] = [24, 36, 48, 60, 72, 84, 96, 108];

pub fn midi_note_label(midi: u8) -> String {
    let name = jambox_core::NOTE_NAMES[(midi % 12) as usize];
    let octave = (midi as i32 / 12) - 1;
    format!("{name}{octave}")
}

pub fn root_octave_index(root_midi: u8) -> usize {
    let c = (root_midi / 12) * 12;
    ROOT_OCTAVE_MIDI
        .iter()
        .position(|&m| m == c)
        .unwrap_or(2) // C3
}

pub fn clamp_root_midi(note: u8) -> u8 {
    note.clamp(ROOT_OCTAVE_MIDI[0], ROOT_OCTAVE_MIDI[ROOT_OCTAVE_MIDI.len() - 1])
}

pub fn program(index: usize) -> KaossProgram {
    KAOSS_PROGRAMS[index % KAOSS_PROGRAMS.len()]
}

pub fn gate(index: usize) -> GatePattern {
    GATE_PATTERNS[index % GATE_PATTERNS.len()]
}

/// Pre–GATE 1/4 sessions stored off=0, 1/8=1, 1/16=2, trip=3.
pub fn migrate_legacy_gate_index(index: usize) -> usize {
    match index {
        0 => 0, // off
        1 => 2, // was 1/8 → now index 2
        2 => 3, // was 1/16
        3 => 4, // was trip
        n => n.min(GATE_PATTERNS.len() - 1),
    }
}

fn program_matches(p: &KaossProgram, show_all: bool, filter: KaossProgramFilter) -> bool {
    (show_all || p.curated) && filter.matches(p.category())
}

pub fn program_count(show_all: bool, filter: KaossProgramFilter) -> usize {
    KAOSS_PROGRAMS
        .iter()
        .filter(|p| program_matches(p, show_all, filter))
        .count()
}

pub fn program_at(show_all: bool, filter: KaossProgramFilter, index: usize) -> KaossProgram {
    let matched: Vec<_> = KAOSS_PROGRAMS
        .iter()
        .filter(|p| program_matches(p, show_all, filter))
        .collect();
    if matched.is_empty() {
        return program(0);
    }
    *matched[index % matched.len()]
}

/// Absolute table index → visible picker row under the current filter.
pub fn program_picker_index(
    show_all: bool,
    filter: KaossProgramFilter,
    abs_index: usize,
) -> Option<usize> {
    let abs_index = abs_index % KAOSS_PROGRAMS.len();
    KAOSS_PROGRAMS
        .iter()
        .enumerate()
        .filter(|(_, p)| program_matches(p, show_all, filter))
        .position(|(i, _)| i == abs_index)
}

pub fn scale_count(show_all: bool) -> usize {
    if show_all {
        jambox_core::KAOSS_SCALES.len()
    } else {
        jambox_core::KAOSS_SCALES
            .iter()
            .filter(|s| s.curated)
            .count()
    }
}

pub fn scale_at(show_all: bool, index: usize) -> jambox_core::KaossScale {
    if show_all {
        jambox_core::kaoss_scale(index)
    } else {
        let curated: Vec<_> = jambox_core::KAOSS_SCALES
            .iter()
            .filter(|s| s.curated)
            .collect();
        *curated[index % curated.len()]
    }
}

pub fn scale_picker_index(show_all: bool, scale_index: u8) -> usize {
    let scale = jambox_core::kaoss_scale(scale_index as usize);
    if show_all {
        scale_index as usize
    } else {
        jambox_core::KAOSS_SCALES
            .iter()
            .enumerate()
            .filter(|(_, s)| s.curated)
            .position(|(_, s)| s.id == scale.id)
            .unwrap_or(0)
    }
}

pub fn picker_count(kind: KaossPicker, show_all: bool, filter: KaossProgramFilter) -> usize {
    match kind {
        KaossPicker::Program => program_count(show_all, filter),
        KaossPicker::Scale => scale_count(show_all),
        KaossPicker::Key => 12,
        // Start C1..C8, then width 1..4 OCT.
        KaossPicker::Octave => ROOT_OCTAVE_MIDI.len() + OCTAVE_LABELS.len(),
        KaossPicker::Gate => GATE_PATTERNS.len(),
    }
}

pub fn picker_label(
    kind: KaossPicker,
    index: usize,
    show_all: bool,
    filter: KaossProgramFilter,
) -> String {
    match kind {
        KaossPicker::Program => program_at(show_all, filter, index).label.to_string(),
        KaossPicker::Scale => scale_at(show_all, index).label.to_string(),
        KaossPicker::Key => jambox_core::NOTE_NAMES[index % 12].to_string(),
        KaossPicker::Octave => {
            if index < ROOT_OCTAVE_MIDI.len() {
                midi_note_label(ROOT_OCTAVE_MIDI[index])
            } else {
                OCTAVE_LABELS[(index - ROOT_OCTAVE_MIDI.len()) % OCTAVE_LABELS.len()].to_string()
            }
        }
        KaossPicker::Gate => gate(index).label.to_string(),
    }
}

pub fn picker_cell(
    bounds: crate::layout::Rect,
    kind: KaossPicker,
    index: usize,
    show_all: bool,
    filter: KaossProgramFilter,
) -> crate::layout::Rect {
    let n = picker_count(kind, show_all, filter).max(1);
    let cols = match kind {
        KaossPicker::Key | KaossPicker::Octave | KaossPicker::Gate => 4,
        KaossPicker::Program => 3,
        KaossPicker::Scale => 4,
    };
    let rows = ((n as i32) + cols - 1) / cols;
    let gw = bounds.w / cols;
    let gh = bounds.h / rows.max(1);
    let col = (index as i32) % cols;
    let row = (index as i32) / cols;
    crate::layout::Rect {
        x: bounds.x + col * gw + 3,
        y: bounds.y + row * gh + 3,
        w: gw - 6,
        h: gh - 6,
    }
}

pub fn gate_period_sec(gate: GatePattern, bpm: f32) -> f64 {
    if gate.beats <= 0.0 {
        return 0.0;
    }
    let bpm = bpm.clamp(40.0, 240.0) as f64;
    (60.0 / bpm) * gate.beats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_all_expands_factory_scales() {
        let starter = scale_count(false);
        let all = scale_count(true);
        assert_eq!(starter, 13);
        assert_eq!(all, jambox_core::KAOSS_SCALES.len());
        assert!(all > starter);
        assert_eq!(scale_at(false, 0).id, "chromatic");
        assert_eq!(scale_at(true, 4).id, "phrygian");
        assert_eq!(scale_at(true, 25).id, "pelog");
    }

    #[test]
    fn scale_picker_index_tracks_curated_subset() {
        let ionian = jambox_core::kaoss_scale_index_by_id("ionian");
        assert_eq!(scale_picker_index(false, ionian), 1);
        assert_eq!(scale_picker_index(true, ionian), ionian as usize);
    }

    #[test]
    fn gate_set_includes_quarter() {
        assert_eq!(GATE_PATTERNS.len(), 5);
        assert_eq!(gate(1).id, "4th");
        assert_eq!(gate(1).label, "GATE 1/4");
        assert_eq!(gate(1).beats, 1.0);
        assert_eq!(migrate_legacy_gate_index(1), 2); // old 1/8
        assert_eq!(migrate_legacy_gate_index(3), 4); // old trip
    }

    #[test]
    fn octave_picker_lists_starts_and_widths() {
        let all = KaossProgramFilter::All;
        assert_eq!(picker_count(KaossPicker::Octave, false, all), 12);
        assert_eq!(picker_label(KaossPicker::Octave, 0, false, all), "C1");
        assert_eq!(picker_label(KaossPicker::Octave, 2, false, all), "C3");
        assert_eq!(picker_label(KaossPicker::Octave, 7, false, all), "C8");
        assert_eq!(picker_label(KaossPicker::Octave, 8, false, all), "1 OCT");
        assert_eq!(picker_label(KaossPicker::Octave, 11, false, all), "4 OCT");
        assert_eq!(root_octave_index(48), 2);
        assert_eq!(root_octave_index(108), 7);
        assert_eq!(midi_note_label(60), "C4");
        assert_eq!(midi_note_label(108), "C8");
        assert_eq!(clamp_root_midi(12), 24);
        assert_eq!(clamp_root_midi(108), 108);
        assert_eq!(clamp_root_midi(120), 108);
    }

    #[test]
    fn bend_y_is_zero_at_midline() {
        assert!((y_to_pitch_bend_semis(0.5)).abs() < 1e-4);
        assert!((y_to_pitch_bend_semis(1.0) - PITCH_BEND_RANGE_SEMIS).abs() < 1e-4);
        assert!((y_to_pitch_bend_semis(0.0) + PITCH_BEND_RANGE_SEMIS).abs() < 1e-4);
        assert_eq!(y_to_pitch_bend_midi(0.5), 8192);
        assert!(KAOSS_PROGRAMS.iter().any(|p| p.id == "bend" && p.curated));
        assert!(KAOSS_PROGRAMS.iter().any(|p| {
            p.id == "wah" && p.curated && p.note && p.y_param == "tone_lfo"
        }));
        assert!(KAOSS_PROGRAMS.iter().any(|p| {
            p.id == "wah_fx" && p.curated && !p.note && p.y_param == "tone_lfo"
        }));
        assert!(KAOSS_PROGRAMS.iter().any(|p| {
            p.id == "vib_fx" && p.curated && !p.note && p.y_param == "vib"
        }));
        assert!(KAOSS_PROGRAMS.iter().any(|p| {
            p.id == "bend_fx" && p.curated && !p.note && p.y_param == "pitch_bend"
        }));
        assert!(KAOSS_PROGRAMS.iter().any(|p| {
            p.id == "dub"
                && p.label == "DUB"
                && !p.note
                && p.y_param == "delay_time"
                && p.x_param == Some("tone")
                && !p.curated
        }));
        let flange = KAOSS_PROGRAMS.iter().find(|p| p.id == "flange").unwrap();
        assert!(flange.curated && flange.note);
        assert_eq!(flange.y_param, "flanger_mix");
        assert_eq!(flange.x_param, None);
        let rate = KAOSS_PROGRAMS.iter().find(|p| p.id == "flange_rate").unwrap();
        assert!(rate.curated && !rate.note);
        assert_eq!(rate.x_param, Some("flanger_rate"));
        let sweep = KAOSS_PROGRAMS.iter().find(|p| p.id == "sweep").unwrap();
        assert!(sweep.curated && !sweep.note && sweep.uses_sweep());
        assert_eq!(sweep.x_param, Some("sweep"));
        assert_eq!(sweep.y_param, "sweep");
        let width = KAOSS_PROGRAMS.iter().find(|p| p.id == "width").unwrap();
        assert!(width.curated && !width.note);
        assert_eq!(width.y_param, "width");
        let scream = KAOSS_PROGRAMS.iter().find(|p| p.id == "scream").unwrap();
        assert!(scream.curated && !scream.note && scream.uses_punch_destroy());
        assert_eq!(scream.x_param, Some("isolator"));
        assert_eq!(scream.y_param, "drive");
        let vinyl = KAOSS_PROGRAMS.iter().find(|p| p.id == "vinyl").unwrap();
        assert!(vinyl.curated && !vinyl.note && vinyl.uses_punch_destroy());
        assert_eq!(vinyl.x_param, Some("punch_tape"));
        assert_eq!(vinyl.y_param, "punch_crush");
        let drop = KAOSS_PROGRAMS.iter().find(|p| p.id == "drop").unwrap();
        assert!(drop.curated && !drop.note && drop.uses_punch_destroy());
        assert_eq!(drop.x_param, Some("punch_drop"));
        assert_eq!(drop.y_param, "isolator");
        assert!(
            !KAOSS_PROGRAMS.iter().any(|p| p.id == "slice"),
            "SLICE PROG retired — use punch-link strip"
        );
        let favs = default_prog_favs();
        assert_eq!(favs.len(), KAOSS_FAV_SLOTS);
        assert!(favs.iter().all(|&f| f != KAOSS_FAV_EMPTY));
        assert_eq!(program(favs[3] as usize).id, "scream");
        assert_eq!(program(favs[4] as usize).id, "vinyl");
    }

    #[test]
    fn every_note_y_effect_has_an_fx_path() {
        // Any Y effect a NOTE program can scrub must also be reachable from an
        // FX program (same y_param or matching x_param) so an arp can get it
        // without stacking another pad tone.
        for note_prog in KAOSS_PROGRAMS.iter().filter(|p| p.note) {
            let effect = note_prog.y_param;
            let covered = KAOSS_PROGRAMS.iter().any(|p| {
                !p.note && (p.y_param == effect || p.x_param == Some(effect))
            });
            assert!(
                covered,
                "NOTE program '{}' y_param '{}' has no FX alternate",
                note_prog.id, effect
            );
        }
    }

    #[test]
    fn bend_deadzone_holds_unison_then_ramps() {
        assert!((y_to_pitch_bend_semis(0.5 + BEND_CENTER_DEADZONE * 0.5)).abs() < 1e-4);
        assert!((y_to_pitch_bend_semis(0.5 - BEND_CENTER_DEADZONE * 0.5)).abs() < 1e-4);
        let just_out = y_to_pitch_bend_semis(0.5 + BEND_CENTER_DEADZONE + 0.01);
        assert!(just_out > 0.0 && just_out < 2.0, "smooth ramp, got {just_out}");
        assert!((apply_center_deadzone(0.5 + BEND_CENTER_DEADZONE, BEND_CENTER_DEADZONE) - 0.5).abs() < 1e-4);
        assert!((apply_center_deadzone(1.0, BEND_CENTER_DEADZONE) - 1.0).abs() < 1e-4);
        assert!((apply_center_deadzone(0.0, BEND_CENTER_DEADZONE)).abs() < 1e-4);
    }

    #[test]
    fn vib_deadzone_stays_off_near_bottom() {
        assert_eq!(apply_zero_deadzone(0.0, ZERO_REST_DEADZONE), 0.0);
        assert_eq!(apply_zero_deadzone(ZERO_REST_DEADZONE, ZERO_REST_DEADZONE), 0.0);
        assert_eq!(apply_zero_deadzone(ZERO_REST_DEADZONE * 0.5, ZERO_REST_DEADZONE), 0.0);
        let just_out = apply_zero_deadzone(ZERO_REST_DEADZONE + 0.02, ZERO_REST_DEADZONE);
        assert!(just_out > 0.0 && just_out < 0.15, "smooth ramp, got {just_out}");
        assert!((apply_zero_deadzone(1.0, ZERO_REST_DEADZONE) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn note_and_fx_families_are_tagged() {
        let lead = KAOSS_PROGRAMS.iter().find(|p| p.id == "lead").unwrap();
        assert_eq!(lead.family_tag(), "NOTE");
        assert_eq!(lead.picker_color(), 0x458588);
        let echo = KAOSS_PROGRAMS.iter().find(|p| p.id == "echo").unwrap();
        assert_eq!(echo.family_tag(), "FX");
        assert_eq!(echo.picker_color(), 0xd65d0e);
        assert_ne!(lead.chrome_color(), echo.chrome_color());
        let expr = KAOSS_PROGRAMS.iter().find(|p| p.id == "expr").unwrap();
        assert_eq!(expr.family_tag(), "MIDI");
        assert_eq!(expr.category(), KaossProgramCategory::Midi);
        assert_eq!(expr.picker_color(), 0xb16286);
    }

    #[test]
    fn program_categories_split_note_fx_midi() {
        let by_id = |id: &str| KAOSS_PROGRAMS.iter().find(|p| p.id == id).unwrap();
        assert_eq!(by_id("lead").category(), KaossProgramCategory::Note);
        assert_eq!(by_id("bend").category(), KaossProgramCategory::Note);
        assert_eq!(by_id("echo").category(), KaossProgramCategory::Fx);
        assert_eq!(by_id("scream").category(), KaossProgramCategory::Fx);
        // Local stereo / width — not USB CC gestures.
        assert_eq!(by_id("sweep").category(), KaossProgramCategory::Fx);
        assert_eq!(by_id("width").category(), KaossProgramCategory::Fx);
        for id in ["expr", "bright", "press", "glide", "pan", "mod"] {
            assert_eq!(
                by_id(id).category(),
                KaossProgramCategory::Midi,
                "{id} should be MIDI"
            );
        }
        // Every program lands in exactly one bucket; buckets partition the table.
        let notes = program_count(true, KaossProgramFilter::Note);
        let fx = program_count(true, KaossProgramFilter::Fx);
        let midi = program_count(true, KaossProgramFilter::Midi);
        assert_eq!(notes + fx + midi, KAOSS_PROGRAMS.len());
        assert_eq!(program_count(true, KaossProgramFilter::All), KAOSS_PROGRAMS.len());
        assert!(notes >= 8 && fx >= 8 && midi == 6);
        // Default filter is NOTE (Kaossilator-first).
        assert_eq!(KaossProgramFilter::default(), KaossProgramFilter::Note);
        assert_eq!(KaossProgramFilter::from_wire("midi").wire(), "midi");
        // Curated NOTE subset is what the picker opens onto by default.
        assert!(program_count(false, KaossProgramFilter::Note) >= 4);
        assert_eq!(program_at(false, KaossProgramFilter::Note, 0).id, "lead");
        let echo_abs = KAOSS_PROGRAMS.iter().position(|p| p.id == "echo").unwrap();
        assert!(program_picker_index(true, KaossProgramFilter::Note, echo_abs).is_none());
        assert_eq!(
            program_picker_index(true, KaossProgramFilter::Fx, echo_abs),
            Some(
                KAOSS_PROGRAMS
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.category() == KaossProgramCategory::Fx)
                    .position(|(i, _)| i == echo_abs)
                    .unwrap()
            )
        );
    }

    /// SPIKE 1 — FX programs that own time/fb/size but not mix need wet engage.
    #[test]
    fn wet_engage_mixes_for_fx_without_mix_axis() {
        let by_id = |id: &str| KAOSS_PROGRAMS.iter().find(|p| p.id == id).unwrap();
        assert!(by_id("lead").wet_engage_mixes().is_empty(), "NOTE: no engage");
        assert!(by_id("echo").wet_engage_mixes().is_empty(), "Y owns delay_mix");
        assert_eq!(by_id("dub").wet_engage_mixes(), &["delay_mix"]);
        assert_eq!(by_id("reso").wet_engage_mixes(), &["delay_mix"]);
        assert!(by_id("wash").wet_engage_mixes().is_empty(), "Y owns reverb_mix");
        assert!(by_id("flange_rate").wet_engage_mixes().is_empty(), "Y owns flanger_mix");
        assert!(by_id("filter").wet_engage_mixes().is_empty(), "morph/tone only");
        assert!(by_id("expr").wet_engage_mixes().is_empty(), "MIDI gesture");
        assert!(KAOSS_WET_ENGAGE > 0.3 && KAOSS_WET_ENGAGE < 1.0);
        assert!(KAOSS_FX_RELEASE_SEC > 0.05 && KAOSS_FX_RELEASE_SEC < 1.0);
    }
}
