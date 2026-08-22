//! Scale-quantized KAOSS pad owned by the audio engine.
//!
//! The UI sends 0..1 pad coordinates plus a stable gesture id. Pitch, tone, and
//! note ownership live here so a stalled renderer cannot leave a voice hanging.

pub const MAX_TOUCH_VOICES: usize = 5;
pub const DEFAULT_ROOT_MIDI: u8 = 48;
pub const DEFAULT_OCTAVES: u8 = 2;
pub const IONIAN: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];

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
}

impl Default for KaossMapper {
    fn default() -> Self {
        Self::new()
    }
}

impl KaossMapper {
    pub fn new() -> Self {
        let notes = scale_notes(&IONIAN, 0, DEFAULT_ROOT_MIDI, DEFAULT_OCTAVES);
        let n_notes = count_filled(&notes);
        Self {
            notes,
            n_notes,
            voices: [TouchVoice::silent(); MAX_TOUCH_VOICES],
        }
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
}
