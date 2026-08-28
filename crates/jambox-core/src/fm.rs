//! Two-operator FM playground.
//!
//! This is a teaching instrument, not a DX7 clone. One oscillator (the
//! *modulator*) wiggles the pitch of another (the *carrier*). Named recipes
//! pick a starting character; four knobs keep the rest in musical English.
//!
//! Bounded for the Pi 2: 8 voices, sine table lookup, no allocation after
//! [`FmSynth::new`].

use crate::voice::midi_to_hz;

const SINE_SIZE: usize = 2048;
const SINE_MASK: usize = SINE_SIZE - 1;
pub const MAX_FM_VOICES: usize = 8;
pub const FM_RECIPE_COUNT: usize = 8;
const VOICE_AMP: f32 = 0.42;

/// Discrete modulator/carrier frequency ratios, in order of "clang".
pub const CLANG_RATIOS: [f32; 10] = [0.5, 1.0, 1.414, 2.0, 3.0, 3.5, 5.0, 7.0, 11.0, 14.0];
pub const CLANG_LABELS: [&str; 10] = [
    "1/2", "1:1", "√2", "2:1", "3:1", "3.5", "5:1", "7:1", "11", "14",
];

#[derive(Debug, Clone, Copy)]
pub struct FmRecipe {
    pub id: &'static str,
    pub label: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
    /// Knob defaults 0..1: bright, clang, hit, tail.
    pub bright: f32,
    pub clang: f32,
    pub hit: f32,
    pub tail: f32,
    pub feedback: f32,
    /// 0 = modulator decays with the carrier; 1 = tine-like (mod dies first).
    pub mod_decay_bias: f32,
    /// 0 = same attack; 1 = brass-like (modulator opens slowly).
    pub mod_attack_bias: f32,
}

pub const FM_RECIPES: [FmRecipe; FM_RECIPE_COUNT] = [
    FmRecipe {
        id: "bell",
        label: "BELL",
        title: "Bell",
        hint: "Metal clang. Bright = more overtones. Clang picks the strike pitch.",
        bright: 0.72,
        clang: 0.55,
        hit: 0.04,
        tail: 0.48,
        feedback: 0.0,
        mod_decay_bias: 0.65,
        mod_attack_bias: 0.0,
    },
    FmRecipe {
        id: "ep",
        label: "E.PIANO",
        title: "E. piano",
        hint: "Tine piano. The wiggle dies fast; the tone rings. Classic 80s keys.",
        bright: 0.52,
        clang: 0.95,
        hit: 0.02,
        tail: 0.58,
        feedback: 0.0,
        mod_decay_bias: 0.88,
        mod_attack_bias: 0.0,
    },
    FmRecipe {
        id: "bass",
        label: "BASS",
        title: "Bass",
        hint: "Low and round. A little Bright adds growl; keep Clang near 1:1 or 2:1.",
        bright: 0.42,
        clang: 0.15,
        hit: 0.10,
        tail: 0.38,
        feedback: 0.18,
        mod_decay_bias: 0.25,
        mod_attack_bias: 0.0,
    },
    FmRecipe {
        id: "brass",
        label: "BRASS",
        title: "Brass",
        hint: "The wiggle opens slowly, so the note gets brighter after you press.",
        bright: 0.50,
        clang: 0.15,
        hit: 0.58,
        tail: 0.42,
        feedback: 0.08,
        mod_decay_bias: 0.10,
        mod_attack_bias: 0.85,
    },
    FmRecipe {
        id: "flute",
        label: "FLUTE",
        title: "Flute",
        hint: "Almost a pure tone. Bright near zero. Hit is breath.",
        bright: 0.12,
        clang: 0.15,
        hit: 0.42,
        tail: 0.50,
        feedback: 0.04,
        mod_decay_bias: 0.15,
        mod_attack_bias: 0.20,
    },
    FmRecipe {
        id: "organ",
        label: "ORGAN",
        title: "Organ",
        hint: "Holds while you hold. Clang at 2:1 or 3:1 stacks harmonics.",
        bright: 0.34,
        clang: 0.45,
        hit: 0.04,
        tail: 0.88,
        feedback: 0.28,
        mod_decay_bias: 0.05,
        mod_attack_bias: 0.0,
    },
    FmRecipe {
        id: "pluck",
        label: "PLUCK",
        title: "Pluck",
        hint: "Instant attack, short tail. A guitar-ish twang.",
        bright: 0.40,
        clang: 0.33,
        hit: 0.0,
        tail: 0.18,
        feedback: 0.10,
        mod_decay_bias: 0.45,
        mod_attack_bias: 0.0,
    },
    FmRecipe {
        id: "growl",
        label: "GROWL",
        title: "Growl",
        hint: "The wiggle feeds back into itself. Messy on purpose.",
        bright: 0.58,
        clang: 0.15,
        hit: 0.16,
        tail: 0.50,
        feedback: 0.82,
        mod_decay_bias: 0.20,
        mod_attack_bias: 0.10,
    },
];

pub fn fm_recipe(index: usize) -> &'static FmRecipe {
    &FM_RECIPES[index % FM_RECIPE_COUNT]
}

pub fn clang_index(unit: f32) -> usize {
    let n = CLANG_RATIOS.len() as f32;
    ((unit.clamp(0.0, 1.0) * (n - 0.001)) as usize).min(CLANG_RATIOS.len() - 1)
}

pub fn clang_ratio(unit: f32) -> f32 {
    CLANG_RATIOS[clang_index(unit)]
}

pub fn clang_label(unit: f32) -> &'static str {
    CLANG_LABELS[clang_index(unit)]
}

/// Live patch derived from a recipe plus the four playground knobs.
#[derive(Debug, Clone, Copy)]
pub struct FmPatch {
    pub car_ratio: f32,
    pub mod_ratio: f32,
    pub index: f32,
    pub feedback: f32,
    pub car_attack: f32,
    pub car_release: f32,
    pub mod_attack: f32,
    pub mod_release: f32,
}

impl FmPatch {
    pub fn from_controls(recipe: usize, bright: f32, clang: f32, hit: f32, tail: f32) -> Self {
        let rec = fm_recipe(recipe);
        let bright = bright.clamp(0.0, 1.0);
        let hit = hit.clamp(0.0, 1.0);
        let tail = tail.clamp(0.0, 1.0);
        let car_attack = 0.002 * (0.45 / 0.002_f32).powf(hit);
        let car_release = 0.04 * (1.60 / 0.04_f32).powf(tail);
        let mod_attack = car_attack * (1.0 + rec.mod_attack_bias * 6.0);
        let mod_release = car_release * (1.0 - rec.mod_decay_bias * 0.85).max(0.08);
        Self {
            car_ratio: 1.0,
            mod_ratio: clang_ratio(clang),
            index: 0.12 + bright.powf(1.35) * 7.4,
            feedback: rec.feedback,
            car_attack,
            car_release,
            mod_attack,
            mod_release,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FmVoice {
    active: bool,
    channel: u8,
    note: u8,
    car_phase: f32,
    mod_phase: f32,
    car_amp: f32,
    mod_amp: f32,
    car_target: f32,
    mod_target: f32,
    last_mod: f32,
    releasing: bool,
    age: u64,
}

impl FmVoice {
    const fn silent() -> Self {
        Self {
            active: false,
            channel: 0,
            note: 0,
            car_phase: 0.0,
            mod_phase: 0.0,
            car_amp: 0.0,
            mod_amp: 0.0,
            car_target: 0.0,
            mod_target: 0.0,
            last_mod: 0.0,
            releasing: false,
            age: 0,
        }
    }
}

pub struct FmSynth {
    sine: [f32; SINE_SIZE],
    voices: [FmVoice; MAX_FM_VOICES],
    serial: u64,
    recipe: usize,
    bright: f32,
    clang: f32,
    hit: f32,
    tail: f32,
    patch: FmPatch,
}

impl Default for FmSynth {
    fn default() -> Self {
        Self::new()
    }
}

impl FmSynth {
    pub fn new() -> Self {
        let mut sine = [0.0f32; SINE_SIZE];
        let tau = std::f32::consts::TAU;
        for (i, sample) in sine.iter_mut().enumerate() {
            *sample = (i as f32 * tau / SINE_SIZE as f32).sin();
        }
        let recipe = 0;
        let rec = fm_recipe(recipe);
        let patch = FmPatch::from_controls(recipe, rec.bright, rec.clang, rec.hit, rec.tail);
        Self {
            sine,
            voices: [FmVoice::silent(); MAX_FM_VOICES],
            serial: 0,
            recipe,
            bright: rec.bright,
            clang: rec.clang,
            hit: rec.hit,
            tail: rec.tail,
            patch,
        }
    }

    pub fn active_count(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    pub fn recipe(&self) -> usize {
        self.recipe
    }

    pub fn patch(&self) -> FmPatch {
        self.patch
    }

    pub fn set_recipe(&mut self, index: usize) {
        let index = index % FM_RECIPE_COUNT;
        let rec = fm_recipe(index);
        self.recipe = index;
        self.bright = rec.bright;
        self.clang = rec.clang;
        self.hit = rec.hit;
        self.tail = rec.tail;
        self.rebuild_patch();
    }

    pub fn set_bright(&mut self, unit: f32) {
        self.bright = unit.clamp(0.0, 1.0);
        self.rebuild_patch();
    }

    pub fn set_clang(&mut self, unit: f32) {
        self.clang = unit.clamp(0.0, 1.0);
        self.rebuild_patch();
    }

    pub fn set_hit(&mut self, unit: f32) {
        self.hit = unit.clamp(0.0, 1.0);
        self.rebuild_patch();
    }

    pub fn set_tail(&mut self, unit: f32) {
        self.tail = unit.clamp(0.0, 1.0);
        self.rebuild_patch();
    }

    fn rebuild_patch(&mut self) {
        self.patch =
            FmPatch::from_controls(self.recipe, self.bright, self.clang, self.hit, self.tail);
    }

    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(channel, note);
            return;
        }
        self.serial = self.serial.wrapping_add(1);
        let vel = (velocity as f32 / 127.0).clamp(0.05, 1.0);
        let car_target = vel * VOICE_AMP;
        // Modulator sits a bit quieter than the carrier; recipes shape the rest.
        let mod_target = vel * VOICE_AMP;

        if let Some(slot) = self.find_playing(channel, note) {
            let v = &mut self.voices[slot];
            v.car_phase = 0.0;
            v.mod_phase = 0.0;
            v.car_amp = 0.0;
            v.mod_amp = 0.0;
            v.car_target = car_target;
            v.mod_target = mod_target;
            v.last_mod = 0.0;
            v.releasing = false;
            v.age = self.serial;
            return;
        }

        let slot = self.free_slot().unwrap_or_else(|| self.steal_slot());
        self.voices[slot] = FmVoice {
            active: true,
            channel,
            note,
            car_phase: 0.0,
            mod_phase: 0.0,
            car_amp: 0.0,
            mod_amp: 0.0,
            car_target,
            mod_target,
            last_mod: 0.0,
            releasing: false,
            age: self.serial,
        };
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        if let Some(slot) = self.find_playing(channel, note) {
            let v = &mut self.voices[slot];
            v.releasing = true;
            v.car_target = 0.0;
            v.mod_target = 0.0;
        }
    }

    pub fn all_notes_off(&mut self) {
        for v in self.voices.iter_mut() {
            if v.active {
                v.releasing = true;
                v.car_target = 0.0;
                v.mod_target = 0.0;
            }
        }
    }

    pub fn silence(&mut self) {
        self.voices = [FmVoice::silent(); MAX_FM_VOICES];
    }

    /// Mix every active voice into `out` (additive).
    pub fn render(&mut self, out: &mut [f32], sample_rate: f32, pitch_mul: f32) {
        let sr = sample_rate.max(8000.0);
        let patch = self.patch;
        let car_atk = env_step(patch.car_attack, sr);
        let car_rel = env_step(patch.car_release, sr);
        let mod_atk = env_step(patch.mod_attack, sr);
        let mod_rel = env_step(patch.mod_release, sr);
        let index_scale = patch.index * (SINE_SIZE as f32 / std::f32::consts::TAU);
        let fb_scale = patch.feedback * (SINE_SIZE as f32 * 0.42);
        let size = SINE_SIZE as f32;

        for v in self.voices.iter_mut() {
            if !v.active {
                continue;
            }
            let hz = midi_to_hz(v.note) * pitch_mul.max(0.01) as f64;
            let car_inc = (hz * patch.car_ratio as f64 * SINE_SIZE as f64 / sr as f64) as f32;
            let mod_inc = (hz * patch.mod_ratio as f64 * SINE_SIZE as f64 / sr as f64) as f32;

            for sample in out.iter_mut() {
                v.car_amp = step_env(v.car_amp, v.car_target, v.releasing, car_atk, car_rel);
                v.mod_amp = step_env(v.mod_amp, v.mod_target, v.releasing, mod_atk, mod_rel);

                let mod_lookup = v.mod_phase + v.last_mod * fb_scale;
                let m = sine_at(&self.sine, mod_lookup) * v.mod_amp;
                v.last_mod = m;
                let c = sine_at(&self.sine, v.car_phase + m * index_scale) * v.car_amp;
                *sample += c;

                v.car_phase += car_inc;
                v.mod_phase += mod_inc;
                if v.car_phase >= size {
                    v.car_phase -= size * (v.car_phase / size).floor();
                }
                if v.mod_phase >= size {
                    v.mod_phase -= size * (v.mod_phase / size).floor();
                }
            }

            if v.releasing && v.car_amp < 0.0008 && v.mod_amp < 0.0008 {
                *v = FmVoice::silent();
            }
        }
    }

    /// One cycle of the current patch at full sustain — for the on-screen scope.
    pub fn preview_cycle(patch: FmPatch, out: &mut [f32]) {
        if out.is_empty() {
            return;
        }
        let n = out.len() as f32;
        let car_inc = SINE_SIZE as f32 / n;
        let mod_inc = car_inc * patch.mod_ratio / patch.car_ratio.max(0.01);
        let index_scale = patch.index * (SINE_SIZE as f32 / std::f32::consts::TAU);
        let fb_scale = patch.feedback * (SINE_SIZE as f32 * 0.42);
        let mut sine = [0.0f32; SINE_SIZE];
        let tau = std::f32::consts::TAU;
        for (i, sample) in sine.iter_mut().enumerate() {
            *sample = (i as f32 * tau / SINE_SIZE as f32).sin();
        }
        let mut car_phase = 0.0f32;
        let mut mod_phase = 0.0f32;
        let mut last = 0.0f32;
        let size = SINE_SIZE as f32;
        for sample in out.iter_mut() {
            let m = sine_at(&sine, mod_phase + last * fb_scale);
            last = m;
            *sample = sine_at(&sine, car_phase + m * index_scale);
            car_phase += car_inc;
            mod_phase += mod_inc;
            if car_phase >= size {
                car_phase -= size * (car_phase / size).floor();
            }
            if mod_phase >= size {
                mod_phase -= size * (mod_phase / size).floor();
            }
        }
    }

    fn find_playing(&self, channel: u8, note: u8) -> Option<usize> {
        self.voices
            .iter()
            .position(|v| v.active && !v.releasing && v.channel == channel && v.note == note)
    }

    fn free_slot(&self) -> Option<usize> {
        self.voices.iter().position(|v| !v.active)
    }

    fn steal_slot(&self) -> usize {
        let mut best = 0usize;
        let mut best_key = (false, f32::MAX, u64::MAX);
        for (i, v) in self.voices.iter().enumerate() {
            let key = (!v.releasing, v.car_amp, v.age);
            if key < best_key {
                best_key = key;
                best = i;
            }
        }
        best
    }
}

#[inline]
fn sine_at(table: &[f32; SINE_SIZE], phase: f32) -> f32 {
    let p = phase.rem_euclid(SINE_SIZE as f32);
    let i = p as usize;
    let frac = p - i as f32;
    let a = table[i & SINE_MASK];
    let b = table[(i + 1) & SINE_MASK];
    a + (b - a) * frac
}

#[inline]
fn env_step(seconds: f32, sample_rate: f32) -> f32 {
    1.0 / (seconds.max(0.001) * sample_rate).max(1.0)
}

#[inline]
fn step_env(amp: f32, target: f32, releasing: bool, attack: f32, release: f32) -> f32 {
    if target > amp {
        (amp + attack * target.max(0.05)).min(target)
    } else {
        let step = if releasing { release } else { attack };
        let next = amp - step * amp.max(0.02);
        if next <= target {
            target
        } else {
            next
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipes_have_unique_ids() {
        let mut seen = std::collections::BTreeSet::new();
        for rec in FM_RECIPES {
            assert!(seen.insert(rec.id), "duplicate recipe {}", rec.id);
            assert!(!rec.label.is_empty());
            assert!(!rec.hint.is_empty());
        }
        assert_eq!(seen.len(), FM_RECIPE_COUNT);
    }

    #[test]
    fn bell_note_makes_sound_and_releases() {
        let mut fm = FmSynth::new();
        fm.set_recipe(0);
        fm.note_on(0, 72, 120);
        assert_eq!(fm.active_count(), 1);
        let mut buf = vec![0.0f32; 2048];
        fm.render(&mut buf, 48_000.0, 1.0);
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01, "FM bell should be audible, peak={peak}");

        fm.note_off(0, 72);
        for _ in 0..80 {
            buf.iter_mut().for_each(|s| *s = 0.0);
            fm.render(&mut buf, 48_000.0, 1.0);
        }
        assert_eq!(fm.active_count(), 0);
    }

    #[test]
    fn clang_steps_through_musical_ratios() {
        assert_eq!(clang_ratio(0.0), 0.5);
        assert_eq!(clang_ratio(0.15), 1.0);
        assert_eq!(clang_ratio(0.55), 3.5);
        assert_eq!(clang_ratio(1.0), 14.0);
    }

    #[test]
    fn preview_cycle_is_nonzero_for_bright_patch() {
        let patch = FmPatch::from_controls(0, 0.8, 0.55, 0.0, 0.5);
        let mut cycle = [0.0f32; 128];
        FmSynth::preview_cycle(patch, &mut cycle);
        let peak = cycle.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.2);
    }

    #[test]
    fn polyphony_is_capped() {
        let mut fm = FmSynth::new();
        for n in 0..MAX_FM_VOICES + 4 {
            fm.note_on(0, 48 + n as u8, 100);
        }
        assert_eq!(fm.active_count(), MAX_FM_VOICES);
    }
}
