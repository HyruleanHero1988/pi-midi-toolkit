//! Scale-quantized KAOSS pad owned by the audio engine.
//!
//! The UI sends 0..1 pad coordinates plus a stable gesture id. Pitch, tone, and
//! note ownership live here so a stalled renderer cannot leave a voice hanging.

pub const MAX_TOUCH_VOICES: usize = 5;
pub const DEFAULT_ROOT_MIDI: u8 = 48;
pub const DEFAULT_OCTAVES: u8 = 2;
pub const IONIAN: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
// IONIAN remains the named alias for the curated major scale degrees.

/// Curated Kaossilator-style scales (UI + engine share the same table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KaossScale {
    pub id: &'static str,
    pub label: &'static str,
    pub degrees: &'static [u8],
    pub curated: bool,
}

// Official Kaossilator PRO scale list (p.99) plus PRO+ / KO-2 extras — parity
// with `apps/pidi/pidi/kaoss.py` `_SCALE_DEFS`.
pub const KAOSS_SCALES: &[KaossScale] = &[
    KaossScale {
        id: "off",
        label: "OFF",
        degrees: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        curated: false,
    },
    KaossScale {
        id: "chromatic",
        label: "CHROMATIC",
        degrees: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        curated: true,
    },
    KaossScale {
        id: "ionian",
        label: "MAJOR",
        degrees: &IONIAN,
        curated: true,
    },
    KaossScale {
        id: "dorian",
        label: "DORIAN",
        degrees: &[0, 2, 3, 5, 7, 9, 10],
        curated: true,
    },
    KaossScale {
        id: "phrygian",
        label: "PHRYGIAN",
        degrees: &[0, 1, 3, 5, 7, 8, 10],
        curated: false,
    },
    KaossScale {
        id: "lydian",
        label: "LYDIAN",
        degrees: &[0, 2, 4, 6, 7, 9, 11],
        curated: false,
    },
    KaossScale {
        id: "mixolydian",
        label: "MIXOLYDIAN",
        degrees: &[0, 2, 4, 5, 7, 9, 10],
        curated: true,
    },
    KaossScale {
        id: "aeolian",
        label: "MINOR",
        degrees: &[0, 2, 3, 5, 7, 8, 10],
        curated: true,
    },
    KaossScale {
        id: "locrian",
        label: "LOCRIAN",
        degrees: &[0, 1, 3, 5, 6, 8, 10],
        curated: false,
    },
    KaossScale {
        id: "harmonic",
        label: "HARM MINOR",
        degrees: &[0, 2, 3, 5, 7, 8, 11],
        curated: true,
    },
    KaossScale {
        id: "melodic",
        label: "MEL MINOR",
        degrees: &[0, 2, 3, 5, 7, 9, 11],
        curated: false,
    },
    KaossScale {
        id: "major_blues",
        label: "MAJ BLUES",
        degrees: &[0, 3, 4, 7, 9, 10],
        curated: false,
    },
    KaossScale {
        id: "blues",
        label: "BLUES",
        degrees: &[0, 3, 5, 6, 7, 10],
        curated: true,
    },
    KaossScale {
        id: "diminish",
        label: "DIMINISH",
        degrees: &[0, 2, 3, 5, 6, 8, 9, 11],
        curated: false,
    },
    KaossScale {
        id: "combo_dim",
        label: "COMBO DIM",
        degrees: &[0, 1, 3, 4, 6, 7, 9, 10],
        curated: false,
    },
    KaossScale {
        id: "major_pent",
        label: "MAJ PENT",
        degrees: &[0, 2, 4, 7, 9],
        curated: true,
    },
    KaossScale {
        id: "minor_pent",
        label: "MIN PENT",
        degrees: &[0, 3, 5, 7, 10],
        curated: true,
    },
    KaossScale {
        id: "raga_bhairav",
        label: "BHAIRAV",
        degrees: &[0, 1, 4, 5, 7, 8, 11],
        curated: false,
    },
    KaossScale {
        id: "raga_gamanasrama",
        label: "GAMANASRAMA",
        degrees: &[0, 1, 4, 6, 7, 9, 11],
        curated: false,
    },
    KaossScale {
        id: "raga_todi",
        label: "TODI",
        degrees: &[0, 1, 3, 6, 7, 8, 11],
        curated: false,
    },
    KaossScale {
        id: "spanish",
        label: "SPANISH",
        degrees: &[0, 1, 3, 4, 5, 7, 8, 10],
        curated: true,
    },
    KaossScale {
        id: "gypsy",
        label: "GYPSY",
        degrees: &[0, 2, 3, 6, 7, 8, 11],
        curated: false,
    },
    KaossScale {
        id: "arabian",
        label: "ARABIAN",
        degrees: &[0, 2, 4, 5, 6, 8, 10],
        curated: false,
    },
    KaossScale {
        id: "egyptian",
        label: "EGYPTIAN",
        degrees: &[0, 2, 5, 7, 10],
        curated: false,
    },
    KaossScale {
        id: "hawaiian",
        label: "HAWAIIAN",
        degrees: &[0, 2, 3, 7, 9],
        curated: false,
    },
    KaossScale {
        id: "pelog",
        label: "PELOG",
        degrees: &[0, 1, 3, 7, 8],
        curated: false,
    },
    KaossScale {
        id: "miyakobushi",
        label: "MIYAKOBUSHI",
        degrees: &[0, 1, 5, 7, 8],
        curated: false,
    },
    KaossScale {
        id: "ryukyu",
        label: "RYUKYU",
        degrees: &[0, 4, 5, 7, 11],
        curated: true,
    },
    KaossScale {
        id: "chinese",
        label: "CHINESE",
        degrees: &[0, 4, 6, 7, 11],
        curated: false,
    },
    KaossScale {
        id: "bassline",
        label: "BASS LINE",
        degrees: &[0, 7, 10],
        curated: true,
    },
    KaossScale {
        id: "whole",
        label: "WHOLE TONE",
        degrees: &[0, 2, 4, 6, 8, 10],
        curated: true,
    },
    KaossScale {
        id: "min3",
        label: "MIN 3RDS",
        degrees: &[0, 3, 6, 9],
        curated: false,
    },
    KaossScale {
        id: "maj3",
        label: "MAJ 3RDS",
        degrees: &[0, 4, 8],
        curated: false,
    },
    KaossScale {
        id: "fourth",
        label: "4THS",
        degrees: &[0, 5, 10],
        curated: false,
    },
    KaossScale {
        id: "fifth",
        label: "5THS",
        degrees: &[0, 7],
        curated: false,
    },
    KaossScale {
        id: "octave",
        label: "OCTAVE",
        degrees: &[0],
        curated: false,
    },
];

/// Default MAJOR (ionian) index in [`KAOSS_SCALES`].
pub const DEFAULT_KAOSS_SCALE_INDEX: u8 = 2;

pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub fn kaoss_scale(index: usize) -> KaossScale {
    KAOSS_SCALES[index % KAOSS_SCALES.len()]
}

pub fn kaoss_scale_index_by_id(id: &str) -> u8 {
    KAOSS_SCALES
        .iter()
        .position(|s| s.id == id)
        .unwrap_or(DEFAULT_KAOSS_SCALE_INDEX as usize) as u8
}

/// Remap indices from the old 13-scale compact table (pre full factory list).
pub fn migrate_legacy_scale_index(index: u8) -> u8 {
    const LEGACY: [&str; 13] = [
        "chromatic",
        "ionian",
        "dorian",
        "mixolydian",
        "aeolian",
        "harmonic",
        "blues",
        "major_pent",
        "minor_pent",
        "spanish",
        "ryukyu",
        "bassline",
        "whole",
    ];
    LEGACY
        .get(index as usize)
        .map(|id| kaoss_scale_index_by_id(id))
        .unwrap_or(index.min(KAOSS_SCALES.len() as u8 - 1))
}

/// Latest XY for one contact. Copied across the lock-free mailbox.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatestTouch {
    pub owner: u32,
    pub x: f32,
    pub y: f32,
    pub channel: u8,
    pub velocity: u8,
}

impl LatestTouch {
    pub fn clamp(self) -> Self {
        let x = self.x.clamp(0.0, 1.0);
        let y = self.y.clamp(0.0, 1.0);
        Self {
            owner: self.owner,
            x,
            y,
            channel: self.channel & 0x0f,
            velocity: if self.velocity == 0 {
                velocity_at_y(y)
            } else {
                self.velocity.min(127).max(1)
            },
        }
    }
}

/// Equal-width cell index; the last cell includes x = 1.
pub fn note_index_at_x(x: f32, n_notes: usize) -> usize {
    let n = n_notes.max(1);
    let idx = (x.clamp(0.0, 1.0) * n as f32) as usize;
    if idx >= n {
        n - 1
    } else {
        idx
    }
}

/// Soft at the bottom of the pad, full at the top — always audible.
pub fn velocity_at_y(y: f32) -> u8 {
    (72 + (y.clamp(0.0, 1.0) * 55.0).round() as i32).clamp(72, 127) as u8
}

pub fn tone_at_y(y: f32) -> f32 {
    y.clamp(0.0, 1.0)
}

/// MIDI notes in `[root, root + octaves*12]` that sit in the scale.
pub fn scale_notes(degrees: &[u8], key: u8, root_midi: u8, octaves: u8) -> [u8; 32] {
    let mut notes = [0u8; 32];
    let mut len = 0usize;
    let key = key % 12;
    let root = root_midi.min(127);
    let span = octaves.clamp(1, 4);
    let top = (root as u16 + span as u16 * 12).min(127) as u8;
    for n in root..=top {
        if degrees.iter().any(|d| (*d + key) % 12 == n % 12) && len < notes.len() {
            notes[len] = n;
            len += 1;
        }
    }
    if len == 0 {
        notes[0] = root;
    }
    notes
}

pub fn note_at_x(x: f32, notes: &[u8], n_notes: usize) -> u8 {
    if n_notes == 0 {
        return 60;
    }
    notes[note_index_at_x(x, n_notes)]
}

pub fn pack_xy(x: f32, y: f32) -> (u16, u16) {
    (
        (x.clamp(0.0, 1.0) * 65535.0).round() as u16,
        (y.clamp(0.0, 1.0) * 65535.0).round() as u16,
    )
}

pub fn unpack_xy(x: u16, y: u16) -> (f32, f32) {
    (x as f32 / 65535.0, y as f32 / 65535.0)
}

#[derive(Debug, Clone, Copy)]
struct TouchVoice {
    active: bool,
    owner: u32,
    channel: u8,
    note: u8,
}

impl TouchVoice {
    const fn silent() -> Self {
        Self {
            active: false,
            owner: 0,
            channel: 0,
            note: 60,
        }
    }
}

/// Result of applying a touch edge or a coalesced move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchDelta {
    Idle,
    Start {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    Retune {
        channel: u8,
        old_note: u8,
        new_note: u8,
        velocity: u8,
    },
    Stop {
        channel: u8,
        note: u8,
    },
}

pub struct KaossMapper {
    notes: [u8; 32],
    n_notes: usize,
    voices: [TouchVoice; MAX_TOUCH_VOICES],
    scale_index: u8,
    key: u8,
    root_midi: u8,
    octaves: u8,
}

impl Default for KaossMapper {
    fn default() -> Self {
        Self::new()
    }
}

impl KaossMapper {
    pub fn new() -> Self {
        let mut mapper = Self {
            notes: [0; 32],
            n_notes: 0,
            voices: [TouchVoice::silent(); MAX_TOUCH_VOICES],
            scale_index: DEFAULT_KAOSS_SCALE_INDEX,
            key: 0,
            root_midi: DEFAULT_ROOT_MIDI,
            octaves: DEFAULT_OCTAVES,
        };
        mapper.rebuild_notes();
        mapper
    }

    pub fn scale_index(&self) -> u8 {
        self.scale_index
    }

    pub fn key(&self) -> u8 {
        self.key
    }

    pub fn configure(&mut self, scale_index: u8, key: u8, root_midi: u8, octaves: u8) {
        self.scale_index = scale_index % KAOSS_SCALES.len() as u8;
        self.key = key % 12;
        self.root_midi = root_midi.min(127);
        self.octaves = octaves.clamp(1, 4);
        self.rebuild_notes();
    }

    fn rebuild_notes(&mut self) {
        let scale = kaoss_scale(self.scale_index as usize);
        self.notes = scale_notes(scale.degrees, self.key, self.root_midi, self.octaves);
        self.n_notes = count_filled(&self.notes);
    }

    pub fn note_at(&self, x: f32) -> u8 {
        note_at_x(x, &self.notes[..self.n_notes], self.n_notes)
    }

    pub fn active_count(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    pub fn owners(&self) -> impl Iterator<Item = u32> + '_ {
        self.voices
            .iter()
            .filter(|v| v.active)
            .map(|v| v.owner)
    }

    pub fn down(
        &mut self,
        owner: u32,
        x: f32,
        y: f32,
        channel: u8,
        velocity: u8,
    ) -> TouchDelta {
        self.up(owner);
        let slot = self
            .voices
            .iter()
            .position(|v| !v.active)
            .unwrap_or(0);
        let note = self.note_at(x);
        let velocity = if velocity == 0 {
            velocity_at_y(y)
        } else {
            velocity.min(127).max(1)
        };
        let channel = channel & 0x0f;
        self.voices[slot] = TouchVoice {
            active: true,
            owner,
            channel,
            note,
        };
        TouchDelta::Start {
            channel,
            note,
            velocity,
        }
    }

    pub fn follow(&mut self, owner: u32, x: f32, y: f32, velocity: u8) -> TouchDelta {
        let Some(slot) = self
            .voices
            .iter()
            .position(|v| v.active && v.owner == owner)
        else {
            return TouchDelta::Idle;
        };
        let new_note = self.note_at(x);
        let channel = self.voices[slot].channel;
        let old_note = self.voices[slot].note;
        let velocity = if velocity == 0 {
            velocity_at_y(y)
        } else {
            velocity.min(127).max(1)
        };
        if new_note == old_note {
            return TouchDelta::Idle;
        }
        self.voices[slot].note = new_note;
        TouchDelta::Retune {
            channel,
            old_note,
            new_note,
            velocity,
        }
    }

    pub fn up(&mut self, owner: u32) -> TouchDelta {
        for voice in &mut self.voices {
            if voice.active && voice.owner == owner {
                let channel = voice.channel;
                let note = voice.note;
                *voice = TouchVoice::silent();
                return TouchDelta::Stop { channel, note };
            }
        }
        TouchDelta::Idle
    }

    pub fn stop_all(&mut self) -> [TouchDelta; MAX_TOUCH_VOICES] {
        let mut out = [TouchDelta::Idle; MAX_TOUCH_VOICES];
        for (i, voice) in self.voices.iter_mut().enumerate() {
            if voice.active {
                out[i] = TouchDelta::Stop {
                    channel: voice.channel,
                    note: voice.note,
                };
                *voice = TouchVoice::silent();
            }
        }
        out
    }
}

fn count_filled(notes: &[u8; 32]) -> usize {
    // Default ionian C3..C5 includes C3=48 ... C5=72, never 0.
    let mut n = 0;
    let mut seen_nonzero = false;
    for (i, note) in notes.iter().enumerate() {
        if *note != 0 {
            seen_nonzero = true;
            n = i + 1;
        } else if seen_nonzero {
            break;
        } else {
            n = i + 1; // MIDI 0 is theoretically possible as the first note
        }
    }
    n.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ionian_c_spans_two_octaves() {
        let notes = scale_notes(&IONIAN, 0, 48, 2);
        let n = count_filled(&notes);
        assert_eq!(notes[0], 48);
        assert_eq!(notes[n - 1], 72);
        assert_eq!(n, 15);
    }

    #[test]
    fn ionian_c8_one_octave() {
        let notes = scale_notes(&IONIAN, 0, 108, 1);
        let n = count_filled(&notes);
        assert_eq!(notes[0], 108); // C8
        assert_eq!(notes[n - 1], 120); // C9
        assert_eq!(n, 8);
    }

    #[test]
    fn x_zero_is_the_left_cell() {
        let mapper = KaossMapper::new();
        assert_eq!(mapper.note_at(0.0), 48);
        assert_eq!(mapper.note_at(1.0), 72);
    }

    #[test]
    fn a_move_to_a_new_cell_retunes() {
        let mut mapper = KaossMapper::new();
        assert!(matches!(
            mapper.down(1, 0.0, 0.5, 0, 100),
            TouchDelta::Start { note: 48, .. }
        ));
        let delta = mapper.follow(1, 1.0, 0.5, 100);
        assert!(matches!(
            delta,
            TouchDelta::Retune {
                old_note: 48,
                new_note: 72,
                ..
            }
        ));
    }

    #[test]
    fn lift_releases_only_that_gesture() {
        let mut mapper = KaossMapper::new();
        mapper.down(1, 0.0, 0.5, 0, 100);
        mapper.down(2, 1.0, 0.5, 0, 100);
        assert_eq!(mapper.active_count(), 2);
        assert!(matches!(mapper.up(1), TouchDelta::Stop { note: 48, .. }));
        assert_eq!(mapper.active_count(), 1);
        assert!(matches!(mapper.up(2), TouchDelta::Stop { note: 72, .. }));
        assert_eq!(mapper.active_count(), 0);
    }

    #[test]
    fn follow_without_down_is_idle() {
        let mut mapper = KaossMapper::new();
        assert_eq!(mapper.follow(9, 0.5, 0.5, 100), TouchDelta::Idle);
    }

    #[test]
    fn full_factory_scale_table_matches_tk() {
        assert_eq!(KAOSS_SCALES.len(), 36);
        assert_eq!(
            KAOSS_SCALES.iter().filter(|s| s.curated).count(),
            13
        );
        assert_eq!(kaoss_scale_index_by_id("ionian"), DEFAULT_KAOSS_SCALE_INDEX);
        assert_eq!(
            migrate_legacy_scale_index(1),
            kaoss_scale_index_by_id("ionian")
        );
        assert_eq!(kaoss_scale(4).id, "phrygian");
    }
}
