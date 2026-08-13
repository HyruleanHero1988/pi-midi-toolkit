//! Polyphonic wavetable voices.
//!
//! Voices are grouped by the wavetable they play, because that group is also the
//! FX insert slot: melody FX on `saw` must not wet a dry kit (see `PLAN.md`).

use crate::wavetable::{TABLE_MASK, TABLE_SIZE};

/// Fixed polyphony. Sized for Pi 2; the array is allocated once.
pub const MAX_VOICES: usize = 16;

#[derive(Debug, Clone, Copy)]
struct Voice {
    active: bool,
    channel: u8,
    note: u8,
    /// Wavetable index — also the FX insert slot.
    group: usize,
    phase: f64,
    amp: f32,
    target_amp: f32,
    releasing: bool,
    age: u64,
}

impl Voice {
    const fn silent() -> Self {
        Self {
            active: false,
            channel: 0,
            note: 0,
            group: 0,
            phase: 0.0,
            amp: 0.0,
            target_amp: 0.0,
            releasing: false,
            age: 0,
        }
    }
}

/// Per-block modulation shared by every voice.
#[derive(Debug, Clone, Copy)]
pub struct VoiceContext {
    pub sample_rate: f32,
    /// Multiplier on frequency (pitch bend × vibrato).
    pub pitch_mul: f32,
    pub attack_sec: f32,
    pub release_sec: f32,
}

/// Fixed-size voice allocator and renderer.
pub struct VoicePool {
    voices: [Voice; MAX_VOICES],
    serial: u64,
    /// Per-voice amplitude at velocity 127.
    voice_amp: f32,
}

impl Default for VoicePool {
    fn default() -> Self {
        Self::new()
    }
}

impl VoicePool {
    pub fn new() -> Self {
        Self {
            voices: [Voice::silent(); MAX_VOICES],
            serial: 0,
            voice_amp: 0.48,
        }
    }

    pub fn active_count(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// Start (or retrigger) a note on `group`'s wavetable.
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8, group: usize) {
        if velocity == 0 {
            self.note_off(channel, note);
            return;
        }
        self.serial = self.serial.wrapping_add(1);
        let target = (velocity as f32 / 127.0) * self.voice_amp;

        if let Some(slot) = self.find_playing(channel, note) {
            let v = &mut self.voices[slot];
            v.group = group;
            v.phase = 0.0;
            v.amp = 0.0;
            v.target_amp = target;
            v.releasing = false;
            v.age = self.serial;
            return;
        }

        let slot = self.free_slot().unwrap_or_else(|| self.steal_slot());
        self.voices[slot] = Voice {
            active: true,
            channel,
            note,
            group,
            phase: 0.0,
            amp: 0.0,
            target_amp: target,
            releasing: false,
            age: self.serial,
        };
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        if let Some(slot) = self.find_playing(channel, note) {
            let v = &mut self.voices[slot];
            v.releasing = true;
            v.target_amp = 0.0;
        }
    }

    /// Release everything (panic / all-notes-off).
    pub fn all_notes_off(&mut self) {
        for v in self.voices.iter_mut() {
            if v.active {
                v.releasing = true;
                v.target_amp = 0.0;
            }
        }
    }

    /// Hard stop — no release tail. Used when the stream restarts.
    pub fn silence(&mut self) {
        self.voices = [Voice::silent(); MAX_VOICES];
    }

    fn find_playing(&self, channel: u8, note: u8) -> Option<usize> {
        self.voices
            .iter()
            .position(|v| v.active && !v.releasing && v.channel == channel && v.note == note)
    }

    fn free_slot(&self) -> Option<usize> {
        self.voices.iter().position(|v| !v.active)
    }

    /// Prefer a releasing voice, then the quietest, then the oldest.
    fn steal_slot(&self) -> usize {
        let mut best = 0usize;
        let mut best_key = (false, f32::MAX, u64::MAX);
        for (i, v) in self.voices.iter().enumerate() {
            let key = (!v.releasing, v.amp, v.age);
            if key < best_key {
                best_key = key;
                best = i;
            }
        }
        best
    }

    /// Collect the distinct wavetable groups with sound in them.
    ///
    /// Returns how many entries of `out` were filled — no allocation.
    pub fn active_groups(&self, out: &mut [usize; MAX_VOICES]) -> usize {
        let mut n = 0;
        for v in self.voices.iter().filter(|v| v.active) {
            if !out[..n].contains(&v.group) {
                out[n] = v.group;
                n += 1;
            }
        }
        n
    }

    /// Sum every voice belonging to `group` into `out` (additive).
    ///
    /// Returns true if any voice in the group is still audible.
    pub fn render_group(
        &mut self,
        group: usize,
        table: &[f32; TABLE_SIZE],
        out: &mut [f32],
        ctx: VoiceContext,
    ) -> bool {
        let mut audible = false;
        let attack_coef = one_pole_coef(ctx.attack_sec, ctx.sample_rate);
        let release_coef = one_pole_coef(ctx.release_sec, ctx.sample_rate);

        for v in self.voices.iter_mut() {
            if !v.active || v.group != group {
                continue;
            }
            audible = true;
            let hz = midi_to_hz(v.note) * ctx.pitch_mul as f64;
            let phase_inc = hz * TABLE_SIZE as f64 / ctx.sample_rate as f64;

            for sample in out.iter_mut() {
                let coef = if v.target_amp > v.amp {
                    attack_coef
                } else {
                    release_coef
                };
                v.amp += (v.target_amp - v.amp) * coef;

                let i0 = v.phase as usize & TABLE_MASK;
                let i1 = (i0 + 1) & TABLE_MASK;
                let frac = (v.phase - v.phase.floor()) as f32;
                let s = table[i0] * (1.0 - frac) + table[i1] * frac;
                *sample += s * v.amp;

                v.phase += phase_inc;
                if v.phase >= TABLE_SIZE as f64 {
                    v.phase -= TABLE_SIZE as f64;
                }
            }

            if v.releasing && v.amp < 0.0005 {
                *v = Voice::silent();
            }
        }
        audible
    }
}

#[inline]
fn one_pole_coef(seconds: f32, sample_rate: f32) -> f32 {
    let n = (seconds.max(0.0005) * sample_rate).max(1.0);
    (1.0 / n).min(1.0)
}

#[inline]
pub fn midi_to_hz(note: u8) -> f64 {
    440.0 * 2f64.powf((note as f64 - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wavetable::WaveBank;

    fn ctx() -> VoiceContext {
        VoiceContext {
            sample_rate: 48_000.0,
            pitch_mul: 1.0,
            attack_sec: 0.002,
            release_sec: 0.010,
        }
    }

    #[test]
    fn note_on_makes_sound_and_note_off_decays() {
        let bank = WaveBank::with_builtins();
        let mut pool = VoicePool::new();
        pool.note_on(0, 69, 127, 0);
        assert_eq!(pool.active_count(), 1);

        let mut buf = vec![0.0f32; 1024];
        pool.render_group(0, bank.table(0), &mut buf, ctx());
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01, "voice should be audible, peak={peak}");

        pool.note_off(0, 69);
        for _ in 0..64 {
            buf.iter_mut().for_each(|v| *v = 0.0);
            pool.render_group(0, bank.table(0), &mut buf, ctx());
        }
        assert_eq!(pool.active_count(), 0, "released voice should be reclaimed");
    }

    #[test]
    fn polyphony_is_capped_by_stealing() {
        let mut pool = VoicePool::new();
        for i in 0..(MAX_VOICES as u8 + 8) {
            pool.note_on(0, 40 + i, 100, 0);
        }
        assert_eq!(pool.active_count(), MAX_VOICES);
    }

    #[test]
    fn groups_track_distinct_wavetables() {
        let mut pool = VoicePool::new();
        pool.note_on(0, 60, 100, 0);
        pool.note_on(0, 64, 100, 2);
        pool.note_on(0, 67, 100, 2);
        let mut groups = [0usize; MAX_VOICES];
        let n = pool.active_groups(&mut groups);
        assert_eq!(n, 2);
        assert!(groups[..n].contains(&0) && groups[..n].contains(&2));
    }

    #[test]
    fn render_group_only_touches_its_own_group() {
        let bank = WaveBank::with_builtins();
        let mut pool = VoicePool::new();
        pool.note_on(0, 69, 127, 2);
        let mut buf = vec![0.0f32; 256];
        let audible = pool.render_group(0, bank.table(0), &mut buf, ctx());
        assert!(!audible);
        assert!(buf.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn retrigger_reuses_the_same_slot() {
        let mut pool = VoicePool::new();
        pool.note_on(0, 60, 100, 0);
        pool.note_on(0, 60, 120, 0);
        assert_eq!(pool.active_count(), 1);
    }

    #[test]
    fn a4_runs_at_concert_pitch() {
        assert!((midi_to_hz(69) - 440.0).abs() < 1e-9);
    }
}
