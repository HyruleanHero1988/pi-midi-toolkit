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
        y_param: "vibrato_always",
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
];

/// Match Tk gate set: off / 1/8 / 1/16 / trip.
pub const GATE_PATTERNS: &[GatePattern] = &[
    GatePattern {
        id: "off",
        label: "GATE OFF",
        beats: 0.0,
        duty: 0.0,
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

pub fn program(index: usize) -> KaossProgram {
    KAOSS_PROGRAMS[index % KAOSS_PROGRAMS.len()]
}

pub fn gate(index: usize) -> GatePattern {
    GATE_PATTERNS[index % GATE_PATTERNS.len()]
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

pub fn picker_count(kind: KaossPicker, show_all_programs: bool) -> usize {
    match kind {
        KaossPicker::Program => program_count(show_all_programs),
        KaossPicker::Scale => jambox_core::KAOSS_SCALES.len(),
        KaossPicker::Key => 12,
        KaossPicker::Octave => 4,
        KaossPicker::Gate => GATE_PATTERNS.len(),
    }
}

pub fn picker_label(kind: KaossPicker, index: usize, show_all_programs: bool) -> String {
    match kind {
        KaossPicker::Program => program_at(show_all_programs, index).label.to_string(),
        KaossPicker::Scale => jambox_core::kaoss_scale(index).label.to_string(),
        KaossPicker::Key => jambox_core::NOTE_NAMES[index % 12].to_string(),
        KaossPicker::Octave => OCTAVE_LABELS[index % 4].to_string(),
        KaossPicker::Gate => gate(index).label.to_string(),
    }
}

pub fn picker_cell(
    bounds: crate::layout::Rect,
    kind: KaossPicker,
    index: usize,
    show_all_programs: bool,
) -> crate::layout::Rect {
    let n = picker_count(kind, show_all_programs).max(1);
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
