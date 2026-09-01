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
    /// Clip / SEQ / song playback — isolated from the live tone knob.
    recorded: bool,
    /// Baked brightness for recorded voices (`1` = open / bypass).
    tone: f32,
    tone_lp: f32,
    tone_bp: f32,
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
            recorded: false,
            tone: 1.0,
            tone_lp: 0.0,
            tone_bp: 0.0,
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
    /// Live tone knob (0 dark … 1 open). Recorded voices ignore this.
    pub live_tone: f32,
    pub tone_lfo_amount: f32,
    pub tone_lfo_rate_hz: f32,
    /// Phase at the start of this span; every live voice walks the same LFO.
    pub tone_lfo_phase: f64,
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

    /// Start (or retrigger) a live note on `group`'s wavetable.
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8, group: usize) {
        self.start_note(channel, note, velocity, group, false, 1.0);
    }

    /// Start a clip / SEQ / song note. `tone` is baked brightness (1 = open).
    pub fn note_on_recorded(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u8,
        group: usize,
        tone: f32,
    ) {
        self.start_note(channel, note, velocity, group, true, tone);
    }

    fn start_note(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u8,
        group: usize,
        recorded: bool,
        tone: f32,
    ) {
        if velocity == 0 {
            self.release_note(channel, note, recorded);
            return;
        }
        self.serial = self.serial.wrapping_add(1);
        let target = (velocity as f32 / 127.0) * self.voice_amp;

        if let Some(slot) = self.find_playing(channel, note, recorded) {
            let v = &mut self.voices[slot];
            v.group = group;
            v.tone = tone.clamp(0.0, 1.0);
            v.tone_lp = 0.0;
            v.tone_bp = 0.0;
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
            recorded,
            tone: tone.clamp(0.0, 1.0),
            tone_lp: 0.0,
            tone_bp: 0.0,
            phase: 0.0,
            amp: 0.0,
            target_amp: target,
            releasing: false,
            age: self.serial,
        };
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        self.release_note(channel, note, false);
    }

    pub fn note_off_recorded(&mut self, channel: u8, note: u8) {
        self.release_note(channel, note, true);
    }

    fn release_note(&mut self, channel: u8, note: u8, recorded: bool) {
        if let Some(slot) = self.find_playing(channel, note, recorded) {
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

    fn find_playing(&self, channel: u8, note: u8, recorded: bool) -> Option<usize> {
        self.voices.iter().position(|v| {
            v.active
                && !v.releasing
                && v.recorded == recorded
                && v.channel == channel
                && v.note == note
        })
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
        let mut unused = [0.0f32; 0];
        self.render_group_split(group, table, out, &mut unused, ctx)
    }

    /// Live voices → `live`; clip / SEQ / song voices → `recorded`.
    ///
    /// Recorded voices apply their baked tone here so the live keys-bus filter
    /// never sees them. `recorded` may be empty (tests / live-only render).
    pub fn render_group_split(
        &mut self,
        group: usize,
        table: &[f32; TABLE_SIZE],
        live: &mut [f32],
        recorded: &mut [f32],
        ctx: VoiceContext,
    ) -> bool {
        let mut audible = false;
        let attack_step = linear_env_step(ctx.attack_sec, ctx.sample_rate);
        let release_step = linear_env_step(ctx.release_sec, ctx.sample_rate);
        let n = live.len();

        for v in self.voices.iter_mut() {
            if !v.active || v.group != group {
                continue;
            }
            let dest = if v.recorded && recorded.len() >= n {
                &mut recorded[..n]
            } else {
                &mut live[..n]
            };
            audible = true;
            let hz = midi_to_hz(v.note) * ctx.pitch_mul as f64;
            let phase_inc = hz * TABLE_SIZE as f64 / ctx.sample_rate as f64;
            let use_lfo = !v.recorded && ctx.tone_lfo_amount > 0.01;
            let static_tone = if v.recorded {
                v.tone
            } else {
                ctx.live_tone
            };
            let filter_tone = !use_lfo && static_tone < 0.985;
            let mut tone_lp = v.tone_lp;
            let mut tone_bp = v.tone_bp;
            let mut lfo_phase = ctx.tone_lfo_phase;
            let lfo_inc = std::f64::consts::TAU * ctx.tone_lfo_rate_hz.max(0.01) as f64
                / ctx.sample_rate.max(8000.0) as f64;

            for sample in dest.iter_mut() {
                if v.target_amp > v.amp {
                    v.amp = (v.amp + attack_step * v.target_amp.max(0.05)).min(v.target_amp);
                } else {
                    let ref_amp = if v.releasing {
                        v.amp.max(1e-4)
                    } else {
                        v.amp.max(0.05)
                    };
                    v.amp = (v.amp - release_step * ref_amp).max(v.target_amp);
                }

                let i0 = v.phase as usize & TABLE_MASK;
                let i1 = (i0 + 1) & TABLE_MASK;
                let frac = (v.phase - v.phase.floor()) as f32;
                let mut s = table[i0] * (1.0 - frac) + table[i1] * frac;
                if use_lfo {
                    lfo_phase += lfo_inc;
                    if lfo_phase > std::f64::consts::TAU {
                        lfo_phase %= std::f64::consts::TAU;
                    }
                    let lfo = 0.5 + 0.5 * lfo_phase.sin() as f32;
                    let tone = (ctx.live_tone * (1.0 - ctx.tone_lfo_amount)
                        + lfo * ctx.tone_lfo_amount)
                        .clamp(0.0, 1.0);
                    s = tone_svf_sample(s, tone, &mut tone_lp, &mut tone_bp, ctx.sample_rate);
                } else if filter_tone {
                    s = tone_svf_sample(
                        s,
                        static_tone,
                        &mut tone_lp,
                        &mut tone_bp,
                        ctx.sample_rate,
                    );
                }
                *sample += s * v.amp;

                v.phase += phase_inc;
                if v.phase >= TABLE_SIZE as f64 {
                    v.phase -= TABLE_SIZE as f64;
                }
            }
            v.tone_lp = tone_lp;
            v.tone_bp = tone_bp;

            if v.releasing && v.amp < 0.0005 {
                *v = Voice::silent();
            }
        }
        audible
    }
}

/// Chamberlin SVF, one sample — same coefficients as the live keys-bus tone.
pub(crate) fn tone_svf_sample(
    input: f32,
    tone: f32,
    lp: &mut f32,
    bp: &mut f32,
    sample_rate: f32,
) -> f32 {
    let tone = tone.clamp(0.0, 1.0);
    if tone >= 0.985 {
        *lp = input;
        *bp = 0.0;
        return input;
    }
    let sr = sample_rate.max(8000.0);
    let fc = 90.0 * (8000.0_f32 / 90.0).powf(tone);
    let fc = fc.min(sr * 0.14);
    let f = (2.0 * std::f32::consts::PI * fc / sr).sin();
    let damp = 0.38 + 0.62 * tone;
    *lp += f * *bp;
    let hp = input - *lp - damp * *bp;
    *bp += f * hp;
    *lp
}

#[inline]
fn linear_env_step(seconds: f32, sample_rate: f32) -> f32 {
    1.0 / (seconds.max(0.0005) * sample_rate).max(1.0)
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
            live_tone: 1.0,
            tone_lfo_amount: 0.0,
            tone_lfo_rate_hz: 5.0,
            tone_lfo_phase: 0.0,
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
    fn live_and_recorded_same_note_are_independent() {
        let bank = WaveBank::with_builtins();
        let mut pool = VoicePool::new();
        pool.note_on(0, 60, 100, 0);
        pool.note_on_recorded(0, 60, 100, 0, 1.0);
        assert_eq!(pool.active_count(), 2);

        pool.note_off_recorded(0, 60);
        let mut live = vec![0.0f32; 2048];
        let mut recorded = vec![0.0f32; 2048];
        for _ in 0..64 {
            live.iter_mut().for_each(|s| *s = 0.0);
            recorded.iter_mut().for_each(|s| *s = 0.0);
            pool.render_group_split(0, bank.table(0), &mut live, &mut recorded, ctx());
        }
        assert_eq!(pool.active_count(), 1, "clip note-off must not kill the live note");
        let peak = live.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01, "live melody should still be sounding");
    }

    #[test]
    fn recorded_voices_render_onto_the_recorded_bus() {
        let bank = WaveBank::with_builtins();
        let mut pool = VoicePool::new();
        pool.note_on_recorded(0, 69, 127, 0, 1.0);
        let mut live = vec![0.0f32; 256];
        let mut recorded = vec![0.0f32; 256];
        pool.render_group_split(0, bank.table(0), &mut live, &mut recorded, ctx());
        assert!(live.iter().all(|s| *s == 0.0));
        let peak = recorded.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01);
    }

    #[test]
    fn a4_runs_at_concert_pitch() {
        assert!((midi_to_hz(69) - 440.0).abs() < 1e-9);
    }
}
