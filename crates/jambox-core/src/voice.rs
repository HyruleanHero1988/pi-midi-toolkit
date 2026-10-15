//! Polyphonic wavetable voices.
//!
//! Voices are grouped by the wavetable they play, because that group is also the
//! FX insert slot: melody FX on `saw` must not wet a dry kit (see `PLAN.md`).

use crate::clip::MAX_CLIPS;
use crate::mix::MixSource;
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
    /// Live keys vs a clip slot (MIX trims).
    mix: MixSource,
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
    /// Extra semitones on this voice only (Kaoss Y bend on a recorded take,
    /// or the wheel value latched when a live note is released).
    bend_semis: f32,
    bend_target_semis: f32,
    /// Seconds since note-off. Drives the release pitch glide.
    release_age: f32,
}

impl Voice {
    const fn silent() -> Self {
        Self {
            active: false,
            channel: 0,
            note: 0,
            group: 0,
            mix: MixSource::Live,
            recorded: false,
            tone: 1.0,
            tone_lp: 0.0,
            tone_bp: 0.0,
            phase: 0.0,
            amp: 0.0,
            target_amp: 0.0,
            releasing: false,
            age: 0,
            bend_semis: 0.0,
            bend_target_semis: 0.0,
            release_age: 0.0,
        }
    }
}

/// Per-block modulation shared by every voice.
#[derive(Debug, Clone, Copy)]
pub struct VoiceContext {
    pub sample_rate: f32,
    /// Multiplier on frequency (pitch bend × vibrato).
    pub pitch_mul: f32,
    /// Pitch-wheel part of `pitch_mul` (1 = unison). A releasing live note
    /// keeps its own latched bend and drops this so the wheel can spring home.
    pub wheel_mul: f32,
    /// Signed semitones reached at the end of the release (0 = no glide).
    pub release_drift_semis: f32,
    /// Seconds in this render span — used to slew per-voice Kaoss bend.
    pub bend_slew_dt: f32,
    pub attack_sec: f32,
    pub release_sec: f32,
    /// Live-keys MIX trim. Clip voices use `clip_gains`.
    pub live_gain: f32,
    pub clip_gains: [f32; MAX_CLIPS],
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

    /// Voices that are still gated (not in release).
    pub fn held_count(&self) -> usize {
        self.voices
            .iter()
            .filter(|v| v.active && !v.releasing)
            .count()
    }

    /// Start (or retrigger) a live note on `group`'s wavetable.
    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8, group: usize) {
        self.start_note(channel, note, velocity, group, MixSource::Live, false, 1.0);
    }

    pub fn note_on_mix(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u8,
        group: usize,
        mix: MixSource,
    ) {
        let recorded = matches!(mix, MixSource::Clip(_));
        self.start_note(channel, note, velocity, group, mix, recorded, 1.0);
    }

    /// Start a clip / SEQ / song note. `tone` is baked brightness (1 = open).
    pub fn note_on_recorded(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u8,
        group: usize,
        tone: f32,
        mix: MixSource,
    ) {
        self.start_note(channel, note, velocity, group, mix, true, tone);
    }

    fn start_note(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u8,
        group: usize,
        mix: MixSource,
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
            v.mix = mix;
            v.tone = tone.clamp(0.0, 1.0);
            v.tone_lp = 0.0;
            v.tone_bp = 0.0;
            v.phase = 0.0;
            v.amp = 0.0;
            v.target_amp = target;
            v.releasing = false;
            v.age = self.serial;
            v.bend_semis = 0.0;
            v.bend_target_semis = 0.0;
            v.release_age = 0.0;
            return;
        }

        let slot = self.free_slot().unwrap_or_else(|| self.steal_slot());
        self.voices[slot] = Voice {
            active: true,
            channel,
            note,
            group,
            mix,
            recorded,
            tone: tone.clamp(0.0, 1.0),
            tone_lp: 0.0,
            tone_bp: 0.0,
            phase: 0.0,
            amp: 0.0,
            target_amp: target,
            releasing: false,
            age: self.serial,
            bend_semis: 0.0,
            bend_target_semis: 0.0,
            release_age: 0.0,
        };
    }

    /// Change pitch of a held voice without resetting phase or envelope.
    pub fn retune(&mut self, channel: u8, old_note: u8, new_note: u8, recorded: bool) -> bool {
        if let Some(slot) = self.find_playing(channel, old_note, recorded) {
            self.voices[slot].note = new_note;
            return true;
        }
        false
    }

    pub fn set_voice_tone(&mut self, channel: u8, note: u8, recorded: bool, tone: f32) {
        if let Some(slot) = self.find_playing(channel, note, recorded) {
            self.voices[slot].tone = tone.clamp(0.0, 1.0);
        }
    }

    pub fn set_voice_bend(&mut self, channel: u8, note: u8, recorded: bool, semis: f32) {
        if let Some(slot) = self.find_playing(channel, note, recorded) {
            let semis = semis.clamp(-24.0, 24.0);
            self.voices[slot].bend_target_semis = semis;
            if semis.abs() < 0.01 || self.voices[slot].bend_semis.abs() < 0.01 {
                self.voices[slot].bend_semis = semis;
            }
        }
    }

    /// Release a live note and keep `bend_semis` on it for the decay.
    ///
    /// The channel wheel can then return to center without dragging this tail
    /// back to unison. Recorded notes keep the bend they already stored.
    pub fn note_off_latched(&mut self, channel: u8, note: u8, bend_semis: f32) {
        if let Some(slot) = self.find_playing(channel, note, false) {
            let v = &mut self.voices[slot];
            let bend = bend_semis.clamp(-24.0, 24.0);
            v.bend_semis = bend;
            v.bend_target_semis = bend;
            v.releasing = true;
            v.target_amp = 0.0;
            v.release_age = 0.0;
        }
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

    /// Release live keys only — recorded pad / SEQ voices keep ringing.
    pub fn all_notes_off_live(&mut self) {
        for v in self.voices.iter_mut() {
            if v.active && !v.recorded {
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

    /// Distinct clip slots that currently have recorded voices.
    pub fn active_clip_slots(&self, out: &mut [usize; MAX_VOICES]) -> usize {
        let mut n = 0;
        for v in self.voices.iter().filter(|v| v.active) {
            if let MixSource::Clip(slot) = v.mix {
                let slot = slot as usize;
                if !out[..n].contains(&slot) {
                    out[n] = slot;
                    n += 1;
                }
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
            let dest = if v.recorded {
                if recorded.len() >= n {
                    &mut recorded[..n]
                } else {
                    continue;
                }
            } else {
                &mut live[..n]
            };
            audible = true;
            v.bend_semis = crate::kaoss::slew_bend(v.bend_semis, v.bend_target_semis, ctx.bend_slew_dt);
            let (mut phase_inc, inc_step, release_age) = phase_inc_span(
                v.note,
                v.bend_semis,
                v.releasing,
                v.recorded,
                v.release_age,
                &ctx,
                n,
            );
            v.release_age = release_age;
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
            let g = v.mix.gain(ctx.live_gain, &ctx.clip_gains);

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
                *sample += s * v.amp * g;

                v.phase += phase_inc;
                phase_inc += inc_step;
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

    /// Render recorded voices that belong to one clip slot.
    pub fn render_clip_slot(
        &mut self,
        slot: usize,
        table: &[f32; TABLE_SIZE],
        out: &mut [f32],
        ctx: VoiceContext,
    ) -> bool {
        let want = MixSource::clip(slot);
        let mut unused = [0.0f32; 0];
        self.render_matching(
            |v| v.recorded && v.mix == want,
            table,
            out,
            &mut unused,
            ctx,
        )
    }

    fn render_matching(
        &mut self,
        matches: impl Fn(&Voice) -> bool,
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
            if !v.active || !matches(v) {
                continue;
            }
            let dest = if v.recorded && recorded.len() >= n {
                &mut recorded[..n]
            } else {
                &mut live[..n]
            };
            audible = true;
            v.bend_semis = crate::kaoss::slew_bend(v.bend_semis, v.bend_target_semis, ctx.bend_slew_dt);
            let (mut phase_inc, inc_step, release_age) = phase_inc_span(
                v.note,
                v.bend_semis,
                v.releasing,
                v.recorded,
                v.release_age,
                &ctx,
                n,
            );
            v.release_age = release_age;
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
            let g = v.mix.gain(ctx.live_gain, &ctx.clip_gains);

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
                *sample += s * v.amp * g;

                v.phase += phase_inc;
                phase_inc += inc_step;
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

/// Frequency ratio for a semitone offset. Unison is 1.
pub fn semis_to_ratio(semis: f32) -> f32 {
    if semis.abs() < 0.001 {
        1.0
    } else {
        2f32.powf(semis / 12.0)
    }
}

/// Glide ratio at `age` seconds into the release. 1 at the note-off.
pub fn drift_ratio(age: f32, release_sec: f32, drift_semis: f32) -> f32 {
    if drift_semis.abs() < 0.001 {
        return 1.0;
    }
    let t = (age / release_sec.max(0.02)).clamp(0.0, 1.0);
    semis_to_ratio(drift_semis * t)
}

/// Map a DRIFT slider (0..1, center 0.5) to ±12 semitones.
pub fn release_drift_semis(unit: f32) -> f32 {
    (unit.clamp(0.0, 1.0) - 0.5) * 24.0
}

/// Phase increment at the start of the block, and how much it changes per sample.
///
/// A releasing live note uses its latched bend instead of the channel wheel, then
/// adds the release glide. Held notes keep wheel × vibrato × per-voice bend.
fn phase_inc_span(
    note: u8,
    bend_semis: f32,
    releasing: bool,
    recorded: bool,
    release_age: f32,
    ctx: &VoiceContext,
    n: usize,
) -> (f64, f64, f32) {
    let sr = ctx.sample_rate.max(8000.0);
    let n = n.max(1);
    let dt = n as f32 / sr;
    let base = midi_to_hz(note) * TABLE_SIZE as f64 / sr as f64;
    let voice = semis_to_ratio(bend_semis);
    let wheel = ctx.wheel_mul.max(0.05);
    let drift0 = if releasing {
        drift_ratio(release_age, ctx.release_sec, ctx.release_drift_semis)
    } else {
        1.0
    };
    let drift1 = if releasing {
        drift_ratio(release_age + dt, ctx.release_sec, ctx.release_drift_semis)
    } else {
        1.0
    };
    let body = if releasing && !recorded {
        ctx.pitch_mul / wheel
    } else {
        ctx.pitch_mul
    };
    let inc0 = base * (body * voice * drift0) as f64;
    let inc1 = base * (body * voice * drift1) as f64;
    let step = if n > 1 {
        (inc1 - inc0) / (n - 1) as f64
    } else {
        0.0
    };
    let age = if releasing { release_age + dt } else { release_age };
    (inc0, step, age)
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
            wheel_mul: 1.0,
            release_drift_semis: 0.0,
            bend_slew_dt: 0.0,
            attack_sec: 0.002,
            release_sec: 0.010,
            live_gain: 1.0,
            clip_gains: [1.0; MAX_CLIPS],
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
        pool.note_on_recorded(0, 60, 100, 0, 1.0, MixSource::clip(0));
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
        pool.note_on_recorded(0, 69, 127, 0, 1.0, MixSource::clip(0));
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

    fn zero_crossings(buf: &[f32]) -> usize {
        buf.windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count()
    }

    #[test]
    fn latched_bend_stays_through_the_release() {
        let bank = WaveBank::with_builtins();
        let mut ctx = ctx();
        ctx.attack_sec = 0.001;
        ctx.release_sec = 1.0;
        ctx.pitch_mul = 1.0;
        ctx.wheel_mul = 1.0;
        let mut plain = VoicePool::new();
        plain.note_on(0, 69, 127, 0);
        let mut bent = VoicePool::new();
        bent.note_on(0, 69, 127, 0);
        let mut warm = vec![0.0f32; 2048];
        plain.render_group(0, bank.table(0), &mut warm, ctx);
        bent.render_group(0, bank.table(0), &mut warm, ctx);
        plain.note_off_latched(0, 69, 0.0);
        bent.note_off_latched(0, 69, 12.0);
        let mut a = vec![0.0f32; 2048];
        let mut b = vec![0.0f32; 2048];
        plain.render_group(0, bank.table(0), &mut a, ctx);
        bent.render_group(0, bank.table(0), &mut b, ctx);
        let za = zero_crossings(&a).max(1);
        let zb = zero_crossings(&b);
        let ratio = zb as f32 / za as f32;
        assert!(
            (ratio - 2.0).abs() < 0.25,
            "octave bend should survive note-off, crossings {za} vs {zb}"
        );
    }

    #[test]
    fn release_drift_glides_away_from_the_latched_pitch() {
        let bank = WaveBank::with_builtins();
        let mut ctx = ctx();
        ctx.attack_sec = 0.001;
        ctx.release_sec = 0.50;
        ctx.release_drift_semis = 12.0;
        let mut pool = VoicePool::new();
        pool.note_on(0, 57, 127, 0);
        let mut warm = vec![0.0f32; 2048];
        pool.render_group(0, bank.table(0), &mut warm, ctx);
        pool.note_off_latched(0, 57, 0.0);
        let mut early = vec![0.0f32; 1024];
        pool.render_group(0, bank.table(0), &mut early, ctx);
        let mut buf = vec![0.0f32; 1024];
        // ~0.4s into a 0.5s release: most of the upward glide has happened.
        for _ in 0..18 {
            buf.iter_mut().for_each(|s| *s = 0.0);
            pool.render_group(0, bank.table(0), &mut buf, ctx);
        }
        let z0 = zero_crossings(&early).max(1);
        let z1 = zero_crossings(&buf);
        assert!(
            z1 > z0 + z0 / 3,
            "pitch should rise across the release, crossings {z0} → {z1}"
        );
    }
}
