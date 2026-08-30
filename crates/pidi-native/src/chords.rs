//! Omnichord-style chord buttons, strum strings, palette, and diatonic changes.
//!
//! Layout mirrors Suzuki OM-27 / OM-108:
//! - 12 roots in **circle-of-fifths** order (F C G D A E B F# Db Ab Eb Bb)
//! - three rows: MAJOR / minor / 7th
//! - same-root and neighbour combos for M7, m7, dim, aug, sus4, add9
//! - a vertical **strumplate** of about two octaves of the selected chord
//!
//! The 8-slot **palette** is a harmonic palette: press a stored chord to play it
//! as a block (MOM releases on lift; HOLD latches), or load a named set of
//! **changes** (common progressions) in the current key.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Key,
    Changes,
}

/// Circle-of-fifths columns, left → right, matching Omnichord button order.
pub const ROOTS_FIFTHS: [u8; 12] = [5, 0, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10];
pub const ROOT_NAMES: [&str; 12] = [
    "F", "C", "G", "D", "A", "E", "B", "F#", "Db", "Ab", "Eb", "Bb",
];
pub const KEY_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

pub const QUALITY_ROWS: usize = 3;
pub const PALETTE_SLOTS: usize = 8;
/// Harp strings on the strum plate (matches the drawn lines).
pub const STRUM_STRINGS: usize = 8;
/// Insets within `Layout::chords_strum_play()` — must match `draw_chords`.
pub const STRUM_BAND_TOP_INSET: i32 = 18;
pub const STRUM_BAND_BOTTOM_INSET: i32 = 4;
/// Lowest strum string ≈ C3 (MIDI 48) for a C chord — midrange, not sub-bass.
pub const STRUM_BASE: u8 = 48;
/// Close-position block voicing around C3 (MIDI 48).
pub const BLOCK_BASE: u8 = 48;
/// Octave shift range for block chords + strumplate (0 = factory C3).
pub const OCTAVE_MIN: i8 = -2;
pub const OCTAVE_MAX: i8 = 2;

pub fn block_base_for_octave(octave: i8) -> u8 {
    let o = octave.clamp(OCTAVE_MIN, OCTAVE_MAX) as i16;
    (i16::from(BLOCK_BASE) + o * 12).clamp(24, 84) as u8
}

pub fn strum_base_for_octave(octave: i8) -> u8 {
    let o = octave.clamp(OCTAVE_MIN, OCTAVE_MAX) as i16;
    (i16::from(STRUM_BASE) + o * 12).clamp(24, 84) as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityRow {
    Maj = 0,
    Min = 1,
    Seven = 2,
}

impl QualityRow {
    pub const ALL: [QualityRow; 3] = [QualityRow::Maj, QualityRow::Min, QualityRow::Seven];

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Maj => "MAJ",
            Self::Min => "min",
            Self::Seven => "7",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordQuality {
    Maj,
    Min,
    Dom7,
    Maj7,
    Min7,
    Dim,
    Aug,
    Sus4,
    Add9,
}

impl ChordQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Maj => "",
            Self::Min => "m",
            Self::Dom7 => "7",
            Self::Maj7 => "M7",
            Self::Min7 => "m7",
            Self::Dim => "dim",
            Self::Aug => "aug",
            Self::Sus4 => "sus4",
            Self::Add9 => "add9",
        }
    }

    /// Pitch-class intervals from the root. Last slot `None` for triads.
    pub fn intervals(self) -> [Option<u8>; 4] {
        match self {
            Self::Maj => [Some(0), Some(4), Some(7), None],
            Self::Min => [Some(0), Some(3), Some(7), None],
            Self::Dom7 => [Some(0), Some(4), Some(7), Some(10)],
            Self::Maj7 => [Some(0), Some(4), Some(7), Some(11)],
            Self::Min7 => [Some(0), Some(3), Some(7), Some(10)],
            Self::Dim => [Some(0), Some(3), Some(6), None],
            Self::Aug => [Some(0), Some(4), Some(8), None],
            Self::Sus4 => [Some(0), Some(5), Some(7), None],
            Self::Add9 => [Some(0), Some(4), Some(7), Some(14)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordSpec {
    pub root: u8,
    pub quality: ChordQuality,
}

impl ChordSpec {
    pub const fn new(root: u8, quality: ChordQuality) -> Self {
        Self {
            root: root % 12,
            quality,
        }
    }

    pub fn name(self) -> String {
        format!("{}{}", KEY_NAMES[self.root as usize], self.quality.label())
    }

    /// Close-position MIDI notes for a block-chord hit (palette / hold).
    pub fn block_notes(self) -> [Option<u8>; 4] {
        self.block_notes_at(BLOCK_BASE)
    }

    pub fn block_notes_at(self, base: u8) -> [Option<u8>; 4] {
        voicing_midi(self, base)
    }

    /// 8 harp strings spanning about two octaves, low → high.
    pub fn strum_strings(self) -> [u8; STRUM_STRINGS] {
        self.strum_strings_at(STRUM_BASE)
    }

    pub fn strum_strings_at(self, base: u8) -> [u8; STRUM_STRINGS] {
        strum_strings_at(self, base)
    }

    /// Shift root by semitones (preserves quality) — used when the song key changes.
    pub fn transpose(self, semitones: u8) -> Self {
        Self::new(self.root.wrapping_add(semitones), self.quality)
    }
}

impl fmt::Display for ChordSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

pub fn root_pc_for_col(col: usize) -> u8 {
    ROOTS_FIFTHS[col % 12]
}

pub fn col_for_root_pc(pc: u8) -> usize {
    ROOTS_FIFTHS
        .iter()
        .position(|&r| r == pc % 12)
        .unwrap_or(1)
}

/// Which quality-row buttons should light for a resolved chord on its root column
/// (and neighbour columns for sus4 / add9).
pub fn lit_buttons_for_chord(spec: ChordSpec) -> Vec<(usize, QualityRow)> {
    let col = col_for_root_pc(spec.root);
    let neighbour = col_for_root_pc(fourth_above(spec.root));
    match spec.quality {
        ChordQuality::Maj => vec![(col, QualityRow::Maj)],
        ChordQuality::Min => vec![(col, QualityRow::Min)],
        ChordQuality::Dom7 => vec![(col, QualityRow::Seven)],
        ChordQuality::Maj7 => vec![(col, QualityRow::Maj), (col, QualityRow::Seven)],
        ChordQuality::Min7 => vec![(col, QualityRow::Min), (col, QualityRow::Seven)],
        ChordQuality::Dim => vec![(col, QualityRow::Maj), (col, QualityRow::Min)],
        ChordQuality::Aug => vec![
            (col, QualityRow::Maj),
            (col, QualityRow::Min),
            (col, QualityRow::Seven),
        ],
        ChordQuality::Sus4 => vec![(col, QualityRow::Maj), (neighbour, QualityRow::Seven)],
        ChordQuality::Add9 => vec![(col, QualityRow::Maj), (neighbour, QualityRow::Min)],
    }
}

/// Fourth above `pc` — the column to the left on the Omnichord fifths row.
pub fn fourth_above(pc: u8) -> u8 {
    (pc + 5) % 12
}

/// OM-108 combination rules from currently held buttons `(col, row)`.
pub fn resolve_held(held: &[(usize, QualityRow)]) -> Option<ChordSpec> {
    if held.is_empty() {
        return None;
    }
    // Prefer combos that share a root, then neighbour (sus4 / add9).
    let mut by_root = [0u8; 12];
    for &(col, row) in held {
        let pc = root_pc_for_col(col) as usize;
        by_root[pc] |= 1 << (row as u8);
    }
    // Pass 1: same-root multi-button (OM-108 M7 / m7 / dim / aug).
    for &pc in &ROOTS_FIFTHS {
        let mask = by_root[pc as usize];
        let maj = mask & 1 != 0;
        let min = mask & 2 != 0;
        let seven = mask & 4 != 0;
        let quality = match (maj, min, seven) {
            (true, true, true) => Some(ChordQuality::Aug),
            (true, true, false) => Some(ChordQuality::Dim),
            (true, false, true) => Some(ChordQuality::Maj7),
            (false, true, true) => Some(ChordQuality::Min7),
            _ => None,
        };
        if let Some(quality) = quality {
            return Some(ChordSpec::new(pc, quality));
        }
    }
    // Pass 2: neighbour combos — MAJOR + the 7th/min a fourth above (left column).
    for &pc in &ROOTS_FIFTHS {
        if by_root[pc as usize] & 1 == 0 {
            continue;
        }
        let nmask = by_root[fourth_above(pc) as usize];
        if nmask & 4 != 0 {
            return Some(ChordSpec::new(pc, ChordQuality::Sus4));
        }
        if nmask & 2 != 0 {
            return Some(ChordSpec::new(pc, ChordQuality::Add9));
        }
    }
    // Pass 3: single buttons.
    for &pc in &ROOTS_FIFTHS {
        let mask = by_root[pc as usize];
        if mask == 0 {
            continue;
        }
        let quality = if mask & 1 != 0 {
            ChordQuality::Maj
        } else if mask & 2 != 0 {
            ChordQuality::Min
        } else {
            ChordQuality::Dom7
        };
        return Some(ChordSpec::new(pc, quality));
    }
    None
}

fn pitch_classes(spec: ChordSpec) -> [Option<u8>; 4] {
    let mut out = [None; 4];
    for (i, iv) in spec.quality.intervals().iter().enumerate() {
        out[i] = iv.map(|v| (spec.root + (v % 12)) % 12);
    }
    out
}

pub fn voicing_midi(spec: ChordSpec, base: u8) -> [Option<u8>; 4] {
    let mut out = [None; 4];
    let mut n = 0usize;
    for iv in spec.quality.intervals() {
        let Some(iv) = iv else { continue };
        let pc = (spec.root + (iv % 12)) % 12;
        let mut midi = base + pc;
        if midi < base {
            midi += 12;
        }
        // add9: the 9th lives an octave above the triad.
        if iv >= 12 {
            midi = midi.saturating_add(12);
        }
        if midi > 127 {
            midi -= 12;
        }
        out[n] = Some(midi.min(127));
        n += 1;
    }
    out
}

pub fn strum_strings(spec: ChordSpec) -> [u8; STRUM_STRINGS] {
    strum_strings_at(spec, STRUM_BASE)
}

pub fn strum_strings_at(spec: ChordSpec, base: u8) -> [u8; STRUM_STRINGS] {
    let mut pcs = [0u8; 4];
    let mut n = 0usize;
    for pc in pitch_classes(spec).into_iter().flatten() {
        if n < 4 {
            pcs[n] = pc;
            n += 1;
        }
    }
    if n == 0 {
        pcs[0] = spec.root;
        n = 1;
    }
    // Sort so the plate always climbs: root, then the rest in pitch-class order
    // wrapping after the root (Omnichord "from the root up").
    let mut ordered = [0u8; 4];
    let mut on = 0usize;
    ordered[on] = spec.root;
    on += 1;
    let mut rest: Vec<u8> = pcs[..n]
        .iter()
        .copied()
        .filter(|&p| p != spec.root)
        .collect();
    rest.sort_unstable();
    for p in rest {
        if on < 4 {
            ordered[on] = p;
            on += 1;
        }
    }
    let mut out = [base; STRUM_STRINGS];
    let mut midi = base;
    // Align so the first string is the chord root at or above `base`.
    while midi % 12 != spec.root {
        midi += 1;
    }
    for i in 0..STRUM_STRINGS {
        let pc = ordered[i % on];
        while midi % 12 != pc {
            midi += 1;
        }
        out[i] = midi.min(127);
        midi += 1;
    }
    out
}

/// Normalized strum Y from a pixel in the play rect: 1 = top band (highest string).
pub fn strum_y_from_play_py(play_y: i32, play_h: i32, py: i32) -> f32 {
    let band_h = ((play_h - STRUM_BAND_TOP_INSET - STRUM_BAND_BOTTOM_INSET).max(1) as f32)
        / STRUM_STRINGS as f32;
    let top = (play_y + STRUM_BAND_TOP_INSET) as f32;
    let bottom = top + (STRUM_STRINGS as f32 - 1.0) * band_h;
    if (py as f32) <= top {
        return 1.0;
    }
    if (py as f32) >= bottom {
        return 0.0;
    }
    let t = 1.0 - ((py as f32 - top) / (bottom - top).max(1.0));
    t.clamp(0.0, 1.0)
}

/// Map normalized strum Y (1 = highest string) onto the harp table.
pub fn string_at(y: f32, strings: &[u8; STRUM_STRINGS]) -> u8 {
    let t = y.clamp(0.0, 1.0) * (STRUM_STRINGS.saturating_sub(1)) as f32;
    let i = t.round() as usize;
    strings[i.min(STRUM_STRINGS - 1)]
}

/// One step of a named progression, relative to the chosen key.
#[derive(Debug, Clone, Copy)]
pub struct Degree {
    /// Chromatic offset from the key (0 = I, 5 = IV, 7 = V, 9 = vi, 10 = bVII).
    pub offset: u8,
    pub quality: ChordQuality,
}

#[derive(Debug, Clone, Copy)]
pub struct Progression {
    pub id: &'static str,
    /// Roman-numeral nickname shown on the tile.
    pub label: &'static str,
    /// Musician-facing name.
    pub name: &'static str,
    pub degrees: &'static [Degree],
}

pub const PROGRESSIONS: &[Progression] = &[
    Progression {
        id: "pop",
        label: "I-V-vi-IV",
        name: "Pop",
        degrees: &[
            Degree { offset: 0, quality: ChordQuality::Maj },
            Degree { offset: 7, quality: ChordQuality::Maj },
            Degree { offset: 9, quality: ChordQuality::Min },
            Degree { offset: 5, quality: ChordQuality::Maj },
        ],
    },
    Progression {
        id: "fifties",
        label: "I-vi-IV-V",
        name: "50s",
        degrees: &[
            Degree { offset: 0, quality: ChordQuality::Maj },
            Degree { offset: 9, quality: ChordQuality::Min },
            Degree { offset: 5, quality: ChordQuality::Maj },
            Degree { offset: 7, quality: ChordQuality::Maj },
        ],
    },
    Progression {
        id: "folk",
        label: "I-IV-V",
        name: "Folk",
        degrees: &[
            Degree { offset: 0, quality: ChordQuality::Maj },
            Degree { offset: 5, quality: ChordQuality::Maj },
            Degree { offset: 7, quality: ChordQuality::Maj },
        ],
    },
    Progression {
        id: "jazz",
        label: "ii-V-I",
        name: "Jazz",
        degrees: &[
            Degree { offset: 2, quality: ChordQuality::Min7 },
            Degree { offset: 7, quality: ChordQuality::Dom7 },
            Degree { offset: 0, quality: ChordQuality::Maj7 },
        ],
    },
    Progression {
        id: "rhythm",
        label: "I-vi-ii-V",
        name: "Rhythm",
        degrees: &[
            Degree { offset: 0, quality: ChordQuality::Maj },
            Degree { offset: 9, quality: ChordQuality::Min },
            Degree { offset: 2, quality: ChordQuality::Min },
            Degree { offset: 7, quality: ChordQuality::Maj },
        ],
    },
    Progression {
        id: "axis",
        label: "vi-IV-I-V",
        name: "Sensitive",
        degrees: &[
            Degree { offset: 9, quality: ChordQuality::Min },
            Degree { offset: 5, quality: ChordQuality::Maj },
            Degree { offset: 0, quality: ChordQuality::Maj },
            Degree { offset: 7, quality: ChordQuality::Maj },
        ],
    },
    Progression {
        id: "mixo",
        label: "I-bVII-IV",
        name: "Mixo",
        degrees: &[
            Degree { offset: 0, quality: ChordQuality::Maj },
            Degree { offset: 10, quality: ChordQuality::Maj },
            Degree { offset: 5, quality: ChordQuality::Maj },
        ],
    },
    Progression {
        id: "blues",
        label: "I7-IV7-V7",
        name: "Blues",
        degrees: &[
            Degree { offset: 0, quality: ChordQuality::Dom7 },
            Degree { offset: 5, quality: ChordQuality::Dom7 },
            Degree { offset: 0, quality: ChordQuality::Dom7 },
            Degree { offset: 7, quality: ChordQuality::Dom7 },
        ],
    },
    Progression {
        id: "andalusian",
        label: "i-bVII-bVI-V",
        name: "Andalusian",
        degrees: &[
            Degree { offset: 0, quality: ChordQuality::Min },
            Degree { offset: 10, quality: ChordQuality::Maj },
            Degree { offset: 8, quality: ChordQuality::Maj },
            Degree { offset: 7, quality: ChordQuality::Maj },
        ],
    },
    Progression {
        id: "canon",
        label: "Pachelbel",
        name: "Canon",
        degrees: &[
            Degree { offset: 0, quality: ChordQuality::Maj },
            Degree { offset: 7, quality: ChordQuality::Maj },
            Degree { offset: 9, quality: ChordQuality::Min },
            Degree { offset: 4, quality: ChordQuality::Min },
            Degree { offset: 5, quality: ChordQuality::Maj },
            Degree { offset: 0, quality: ChordQuality::Maj },
            Degree { offset: 5, quality: ChordQuality::Maj },
            Degree { offset: 7, quality: ChordQuality::Maj },
        ],
    },
];

pub fn progression_in_key(prog: &Progression, key: u8) -> [Option<ChordSpec>; PALETTE_SLOTS] {
    let mut out = [None; PALETTE_SLOTS];
    for (i, deg) in prog.degrees.iter().take(PALETTE_SLOTS).enumerate() {
        out[i] = Some(ChordSpec::new(key.wrapping_add(deg.offset) % 12, deg.quality));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_major_from_one_button() {
        let c_col = col_for_root_pc(0);
        let chord = resolve_held(&[(c_col, QualityRow::Maj)]).unwrap();
        assert_eq!(chord.name(), "C");
        let notes = voicing_midi(chord, 48);
        let pcs: Vec<u8> = notes.into_iter().flatten().map(|n| n % 12).collect();
        assert!(pcs.contains(&0) && pcs.contains(&4) && pcs.contains(&7));
    }

    #[test]
    fn om108_combos_on_c() {
        let c = col_for_root_pc(0);
        let f = col_for_root_pc(5);
        assert_eq!(
            resolve_held(&[(c, QualityRow::Maj), (c, QualityRow::Seven)])
                .unwrap()
                .quality,
            ChordQuality::Maj7
        );
        assert_eq!(
            resolve_held(&[(c, QualityRow::Min), (c, QualityRow::Seven)])
                .unwrap()
                .quality,
            ChordQuality::Min7
        );
        assert_eq!(
            resolve_held(&[(c, QualityRow::Maj), (c, QualityRow::Min)])
                .unwrap()
                .quality,
            ChordQuality::Dim
        );
        assert_eq!(
            resolve_held(&[
                (c, QualityRow::Maj),
                (c, QualityRow::Min),
                (c, QualityRow::Seven)
            ])
            .unwrap()
            .quality,
            ChordQuality::Aug
        );
        assert_eq!(
            resolve_held(&[(c, QualityRow::Maj), (f, QualityRow::Seven)])
                .unwrap()
                .quality,
            ChordQuality::Sus4
        );
        assert_eq!(
            resolve_held(&[(c, QualityRow::Maj), (f, QualityRow::Min)])
                .unwrap()
                .quality,
            ChordQuality::Add9
        );
    }

    #[test]
    fn fifths_order_puts_c_next_to_f_and_g() {
        assert_eq!(ROOT_NAMES[0], "F");
        assert_eq!(ROOT_NAMES[1], "C");
        assert_eq!(ROOT_NAMES[2], "G");
        assert_eq!(fourth_above(0), 5); // C → F
    }

    #[test]
    fn strum_climbs_and_stays_in_chord() {
        let c = ChordSpec::new(0, ChordQuality::Maj);
        let strings = c.strum_strings();
        assert!(strings[0] < strings[STRUM_STRINGS - 1]);
        let allowed = [0u8, 4, 7];
        for n in strings {
            assert!(allowed.contains(&(n % 12)), "out of chord: {n}");
        }
        let span = strings[STRUM_STRINGS - 1] as i16 - strings[0] as i16;
        assert!(
            span <= 30,
            "strum should stay ~2 octaves, got {span} semis ({:?})",
            strings
        );
        assert_eq!(strings[0], 48, "default C major starts at C3");
    }

    #[test]
    fn pop_changes_in_g() {
        let prog = PROGRESSIONS.iter().find(|p| p.id == "pop").unwrap();
        let pal = progression_in_key(prog, 7);
        let names: Vec<String> = pal.iter().flatten().map(|c| c.name()).collect();
        assert_eq!(names, ["G", "D", "Em", "C"]);
    }

    #[test]
    fn string_at_top_is_highest() {
        let c = ChordSpec::new(0, ChordQuality::Maj);
        let s = c.strum_strings();
        assert_eq!(string_at(1.0, &s), s[STRUM_STRINGS - 1], "pad top → high");
        assert_eq!(string_at(0.0, &s), s[0], "pad bottom → low");
    }

    #[test]
    fn strum_touch_y_tracks_drawn_bands() {
        let play_y = 120;
        let play_h = 170;
        let band_h =
            ((play_h - STRUM_BAND_TOP_INSET - STRUM_BAND_BOTTOM_INSET).max(1) as f32) / 8.0;
        let top = play_y + STRUM_BAND_TOP_INSET;
        let c = ChordSpec::new(0, ChordQuality::Maj);
        let s = c.strum_strings();
        for band in 0..STRUM_STRINGS {
            let py = (top as f32 + band as f32 * band_h + band_h * 0.5) as i32;
            let y = strum_y_from_play_py(play_y, play_h, py);
            let note = string_at(y, &s);
            let expected = s[STRUM_STRINGS - 1 - band];
            assert_eq!(
                note, expected,
                "band {band} py={py} y={y:.2} expected {} got {}",
                crate::kaoss_ui::midi_note_label(expected),
                crate::kaoss_ui::midi_note_label(note),
            );
        }
    }

    #[test]
    fn transpose_follows_key_change() {
        let c = ChordSpec::new(0, ChordQuality::Maj);
        assert_eq!(c.transpose(7).name(), "G");
        assert_eq!(ChordSpec::new(9, ChordQuality::Min).transpose(7).name(), "Em");
    }

    #[test]
    fn octave_shift_moves_block_and_strum() {
        let c = ChordSpec::new(0, ChordQuality::Maj);
        let low = c.block_notes_at(block_base_for_octave(-1));
        let mid = c.block_notes_at(block_base_for_octave(0));
        assert_eq!(
            mid[0].unwrap() - low[0].unwrap(),
            12,
            "−1 octave drops a twelfth"
        );
        let s0 = c.strum_strings_at(strum_base_for_octave(0));
        let s1 = c.strum_strings_at(strum_base_for_octave(1));
        assert_eq!(s1[0] - s0[0], 12);
    }
}
