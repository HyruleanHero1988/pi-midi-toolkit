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
    pub note: bool,
    pub y_param: &'static str,
    pub x_param: Option<&'static str>,
    pub curated: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct GatePattern {
    pub id: &'static str,
    pub label: &'static str,
    pub beats: f64,
    pub duty: f64,
}

/// Full program table — curated first, then the rest (SHOW ALL).
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
    KaossProgram {
        id: "flange",
        label: "FLANGE",
        note: false,
        y_param: "flanger_mix",
        x_param: Some("flanger_rate"),
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
    KaossProgram {
        id: "sweep",
        label: "SWEEP",
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
    // X = scale pitch; Y = how fast the tone filter oscillates (auto-wah).
    KaossProgram {
        id: "wah",
        label: "WAH",
        note: true,
        y_param: "tone_lfo",
        x_param: None,
        curated: true,
    },
];

/// Full-pad Y travel maps to ± this many semitones (center Y = 0).
pub const PITCH_BEND_RANGE_SEMIS: f32 = 12.0;

/// Pad Y (0 = bottom, 1 = top) → pitch-bend semitones. Midline is unison.
pub fn y_to_pitch_bend_semis(y: f32) -> f32 {
    let centered = (y.clamp(0.0, 1.0) - 0.5) * 2.0; // -1 .. +1
    centered * PITCH_BEND_RANGE_SEMIS
}

/// MIDI 14-bit pitch wheel (8192 = center) for a pad-Y bend.
pub fn y_to_pitch_bend_midi(y: f32) -> u16 {
    let t = (y_to_pitch_bend_semis(y) / PITCH_BEND_RANGE_SEMIS).clamp(-1.0, 1.0);
    (8192.0 + t * 8192.0).round().clamp(0.0, 16383.0) as u16
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
/// Left-edge C of the pad (Kaossilator-style). C1 .. C5.
pub const ROOT_OCTAVE_MIDI: [u8; 5] = [24, 36, 48, 60, 72];

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

pub fn program_count(show_all: bool) -> usize {
    if show_all {
        KAOSS_PROGRAMS.len()
    } else {
        KAOSS_PROGRAMS.iter().filter(|p| p.curated).count()
    }
}

pub fn program_at(show_all: bool, index: usize) -> KaossProgram {
    if show_all {
        program(index)
    } else {
        let curated: Vec<_> = KAOSS_PROGRAMS.iter().filter(|p| p.curated).collect();
        *curated[index % curated.len()]
    }
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

pub fn picker_count(kind: KaossPicker, show_all: bool) -> usize {
    match kind {
        KaossPicker::Program => program_count(show_all),
        KaossPicker::Scale => scale_count(show_all),
        KaossPicker::Key => 12,
        // Start C1..C5, then width 1..4 OCT (Tk "OCTAVE — start + width").
        KaossPicker::Octave => ROOT_OCTAVE_MIDI.len() + OCTAVE_LABELS.len(),
        KaossPicker::Gate => GATE_PATTERNS.len(),
    }
}

pub fn picker_label(kind: KaossPicker, index: usize, show_all: bool) -> String {
    match kind {
        KaossPicker::Program => program_at(show_all, index).label.to_string(),
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
) -> crate::layout::Rect {
    let n = picker_count(kind, show_all).max(1);
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
        assert_eq!(picker_count(KaossPicker::Octave, false), 9);
        assert_eq!(picker_label(KaossPicker::Octave, 0, false), "C1");
        assert_eq!(picker_label(KaossPicker::Octave, 2, false), "C3");
        assert_eq!(picker_label(KaossPicker::Octave, 5, false), "1 OCT");
        assert_eq!(picker_label(KaossPicker::Octave, 8, false), "4 OCT");
        assert_eq!(root_octave_index(48), 2);
        assert_eq!(midi_note_label(60), "C4");
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
    }
}
