//! Kaoss program / picker helpers (behavioral parity with `apps/pidi/pidi/kaoss.py`).

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
    /// `true` = note pad; `false` = FX-only XY (no pitch).
    pub note: bool,
    pub y_param: &'static str,
    pub x_param: Option<&'static str>,
}

/// Curated starter set — same ids as Tk.
pub const KAOSS_PROGRAMS: &[KaossProgram] = &[
    KaossProgram {
        id: "lead",
        label: "LEAD",
        note: true,
        y_param: "tone",
        x_param: None,
    },
    KaossProgram {
        id: "morph",
        label: "MORPH",
        note: true,
        y_param: "morph",
        x_param: None,
    },
    KaossProgram {
        id: "vib",
        label: "VIB",
        note: true,
        y_param: "vib",
        x_param: None,
    },
    KaossProgram {
        id: "filter",
        label: "FILTER",
        note: false,
        y_param: "morph",
        x_param: Some("tone"),
    },
    KaossProgram {
        id: "echo",
        label: "ECHO",
        note: false,
        y_param: "delay_mix",
        x_param: Some("delay_time"),
    },
    KaossProgram {
        id: "drive",
        label: "DRIVE",
        note: false,
        y_param: "reverb_mix",
        x_param: Some("drive"),
    },
];

pub const GATE_LABELS: [&str; 5] = ["GATE OFF", "1/4", "1/8", "1/8T", "1/16"];
pub const OCTAVE_LABELS: [&str; 4] = ["1 OCT", "2 OCT", "3 OCT", "4 OCT"];

pub fn program(index: usize) -> KaossProgram {
    KAOSS_PROGRAMS[index % KAOSS_PROGRAMS.len()]
}

pub fn picker_count(kind: KaossPicker) -> usize {
    match kind {
        KaossPicker::Program => KAOSS_PROGRAMS.len(),
        KaossPicker::Scale => jambox_core::KAOSS_SCALES.len(),
        KaossPicker::Key => 12,
        KaossPicker::Octave => 4,
        KaossPicker::Gate => GATE_LABELS.len(),
    }
}

pub fn picker_label(kind: KaossPicker, index: usize) -> String {
    match kind {
        KaossPicker::Program => program(index).label.to_string(),
        KaossPicker::Scale => jambox_core::kaoss_scale(index).label.to_string(),
        KaossPicker::Key => jambox_core::NOTE_NAMES[index % 12].to_string(),
        KaossPicker::Octave => OCTAVE_LABELS[index % 4].to_string(),
        KaossPicker::Gate => GATE_LABELS[index % GATE_LABELS.len()].to_string(),
    }
}

/// Grid geometry for an overlay picker inside `bounds`.
pub fn picker_cell(
    bounds: crate::layout::Rect,
    kind: KaossPicker,
    index: usize,
) -> crate::layout::Rect {
    let n = picker_count(kind).max(1);
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
