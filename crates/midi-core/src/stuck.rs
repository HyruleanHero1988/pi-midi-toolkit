//! Track sounding notes so we can emit note-offs on disconnect / preset swap.

use crate::event::{Channel, MidiEvent, Note};
use crate::process::{ProcessOutput, MAX_OUT};

/// Bitset of active notes: 16 channels × 128 notes.
#[derive(Debug, Clone)]
pub struct ActiveNotes {
    /// `bits[channel][note / 64]` holds two u64 limbs covering notes 0–127.
    bits: [[u64; 2]; 16],
}

impl Default for ActiveNotes {
    fn default() -> Self {
        Self::new()
    }
}

impl ActiveNotes {
    pub const fn new() -> Self {
        Self {
            bits: [[0; 2]; 16],
        }
    }

    #[inline]
    fn limb(note: Note) -> (usize, u64) {
        let note = (note & 0x7f) as usize;
        (note / 64, 1u64 << (note % 64))
    }

    #[inline]
    pub fn is_active(&self, channel: Channel, note: Note) -> bool {
        let ch = (channel & 0x0f) as usize;
        let (i, mask) = Self::limb(note);
        self.bits[ch][i] & mask != 0
    }

    #[inline]
    pub fn set(&mut self, channel: Channel, note: Note) {
        let ch = (channel & 0x0f) as usize;
        let (i, mask) = Self::limb(note);
        self.bits[ch][i] |= mask;
    }

    #[inline]
    pub fn clear_note(&mut self, channel: Channel, note: Note) {
        let ch = (channel & 0x0f) as usize;
        let (i, mask) = Self::limb(note);
        self.bits[ch][i] &= !mask;
    }

    pub fn clear_all(&mut self) {
        self.bits = [[0; 2]; 16];
    }

    /// Update tracking from a **post-transform** event (what we sent / will send).
    pub fn observe(&mut self, event: MidiEvent) {
        match event {
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } => {
                if velocity == 0 {
                    self.clear_note(channel, note);
                } else {
                    self.set(channel, note);
                }
            }
            MidiEvent::NoteOff { channel, note, .. } => {
                self.clear_note(channel, note);
            }
            _ => {}
        }
    }

    /// Emit note-offs for every active note, then clear. May need multiple calls
    /// if more than [`MAX_OUT`] notes are held (caller loops until empty).
    pub fn drain_note_offs(&mut self) -> ProcessOutput {
        let mut out = ProcessOutput::empty();
        for ch in 0u8..16 {
            for limb_i in 0..2 {
                let mut limb = self.bits[ch as usize][limb_i];
                while limb != 0 {
                    if out.len() >= MAX_OUT {
                        return out;
                    }
                    let bit = limb.trailing_zeros();
                    let note = (limb_i * 64 + bit as usize) as u8;
                    out.push(MidiEvent::NoteOff {
                        channel: ch,
                        note,
                        velocity: 0,
                    });
                    limb &= !(1u64 << bit);
                    self.bits[ch as usize][limb_i] &= !(1u64 << bit);
                }
            }
        }
        out
    }

    /// Convenience: drain until empty into a `Vec` (config/disconnect path only).
    pub fn flush_all_note_offs(&mut self) -> Vec<MidiEvent> {
        let mut all = Vec::new();
        loop {
            let batch = self.drain_note_offs();
            if batch.is_empty() {
                break;
            }
            all.extend(batch.iter());
        }
        all
    }

    pub fn any_active(&self) -> bool {
        self.bits.iter().any(|ch| ch[0] != 0 || ch[1] != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_and_flush() {
        let mut a = ActiveNotes::new();
        a.observe(MidiEvent::NoteOn {
            channel: 2,
            note: 60,
            velocity: 100,
        });
        a.observe(MidiEvent::NoteOn {
            channel: 2,
            note: 64,
            velocity: 100,
        });
        assert!(a.is_active(2, 60));
        a.observe(MidiEvent::NoteOff {
            channel: 2,
            note: 60,
            velocity: 0,
        });
        assert!(!a.is_active(2, 60));
        assert!(a.is_active(2, 64));

        let offs = a.flush_all_note_offs();
        assert_eq!(offs.len(), 1);
        assert_eq!(
            offs[0],
            MidiEvent::NoteOff {
                channel: 2,
                note: 64,
                velocity: 0,
            }
        );
        assert!(!a.any_active());
    }

    #[test]
    fn note_on_vel_zero_clears() {
        let mut a = ActiveNotes::new();
        a.observe(MidiEvent::NoteOn {
            channel: 0,
            note: 10,
            velocity: 1,
        });
        a.observe(MidiEvent::NoteOn {
            channel: 0,
            note: 10,
            velocity: 0,
        });
        assert!(!a.any_active());
    }
}
