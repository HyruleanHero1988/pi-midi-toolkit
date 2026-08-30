//! Four-operator FM playground, patched by drawing.
//!
//! Inspired by LOVE Synthesizers FIRST LOVE: you swipe one operator into
//! another instead of filling in a DX7 matrix. Each operator is a sine that
//! can fold toward a square, and a 4×4 amount matrix (including self = feedback)
//! is the algorithm.
//!
//! Bounded for the Pi 2: 8 voices, sine table lookup, no allocation after
//! [`FmSynth::new`].

use crate::voice::midi_to_hz;

const SINE_SIZE: usize = 2048;
const SINE_MASK: usize = SINE_SIZE - 1;
pub const MAX_FM_VOICES: usize = 8;
pub const FM_RECIPE_COUNT: usize = 8;
pub const FM_OP_COUNT: usize = 4;
const VOICE_AMP: f32 = 0.38;
const INDEX_SCALE: f32 = SINE_SIZE as f32 / std::f32::consts::TAU * 5.5;

/// Discrete operator frequency ratios, in order of "clang".
///
/// Labels stay in ASCII 32..95 so the 5×7 bitmap (and the smooth atlas)
/// can draw them. `√2` rendered as a blank plus "2" and looked identical
/// to the next step, `2:1`.
pub const CLANG_RATIOS: [f32; 10] = [0.5, 1.0, 1.414, 2.0, 3.0, 3.5, 5.0, 7.0, 11.0, 14.0];
pub const CLANG_LABELS: [&str; 10] = [
    "1/2", "1:1", "1.41", "2:1", "3:1", "3.5", "5:1", "7:1", "11", "14",
];
pub const OP_NAMES: [&str; FM_OP_COUNT] = ["A", "B", "C", "D"];
pub const OP_COLORS: [u32; FM_OP_COUNT] = [0xfb4934, 0xfe8019, 0xb8bb26, 0x458588];

#[derive(Debug, Clone, Copy)]
pub struct FmRecipe {
    pub id: &'static str,
    pub label: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
}

pub const FM_RECIPES: [FmRecipe; FM_RECIPE_COUNT] = [
    FmRecipe {
        id: "bell",
        label: "BELL",
        title: "Bell",
        hint: "A and B strike D, then die. The ring is D.",
    },
    FmRecipe {
        id: "ep",
        label: "E.PIANO",
        title: "E. piano",
        hint: "A is the tine tick. B is the warm body.",
    },
    FmRecipe {
        id: "bass",
        label: "BASS",
        hint: "A bites itself and D. Fold A for more growl.",
        title: "Bass",
    },
    FmRecipe {
        id: "brass",
        label: "BRASS",
        title: "Brass",
        hint: "A into B into D. Brightness fades in.",
    },
    FmRecipe {
        id: "flute",
        label: "FLUTE",
        title: "Flute",
        hint: "Mostly D. A little self-wobble is breath.",
    },
    FmRecipe {
        id: "organ",
        label: "ORGAN",
        title: "Organ",
        hint: "A B C D all sound. Stacked sines, almost no FM.",
    },
    FmRecipe {
        id: "pluck",
        label: "PLUCK",
        title: "Pluck",
        hint: "A into B into D, all die. Guitar snap.",
    },
    FmRecipe {
        id: "growl",
        label: "GROWL",
        title: "Growl",
        hint: "A onto itself and D, B into D. Messy.",
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

/// Pack a drawn connection: integer 0..15 is from + to*4, fraction is amount.
pub fn pack_fm_link(from: usize, to: usize, amount: f32) -> f32 {
    let from = from % FM_OP_COUNT;
    let to = to % FM_OP_COUNT;
    (from + to * FM_OP_COUNT) as f32 + amount.clamp(0.02, 0.99)
}

pub fn unpack_fm_link(value: f32) -> (usize, usize, f32) {
    let v = value.max(0.0);
    let idx = v.floor() as usize;
    let amount = v.fract().clamp(0.02, 1.0);
    (idx % FM_OP_COUNT, (idx / FM_OP_COUNT) % FM_OP_COUNT, amount)
}

#[derive(Debug, Clone, Copy)]
pub struct FmOpParams {
    /// 0..1 into [`CLANG_RATIOS`].
    pub ratio: f32,
    /// Mix into the heard output.
    pub audio: f32,
    /// 0 = sine, 1 = folded / pushed toward square.
    pub fold: f32,
    /// 0 = snappy times, 1 = slow open.
    pub env: f32,
    /// Level held after the strike, while the key is down. 0 = dies even if held.
    pub sustain: f32,
}

impl Default for FmOpParams {
    fn default() -> Self {
        Self {
            ratio: 0.15,
            audio: 0.0,
            fold: 0.0,
            env: 0.35,
            sustain: 1.0,
        }
    }
}

impl FmOpParams {
    fn attack_sec(self) -> f32 {
        0.002 * (0.50 / 0.002_f32).powf(self.env.clamp(0.0, 1.0))
    }

    fn decay_sec(self) -> f32 {
        0.022 * (0.40 / 0.022_f32).powf(self.env.clamp(0.0, 1.0))
    }

    fn release_sec(self) -> f32 {
        0.045 * (1.50 / 0.045_f32).powf(self.env.clamp(0.0, 1.0))
    }
}

/// Four operators plus who wiggles whom. `matrix[src][dst]` is the draw amount.
#[derive(Debug, Clone, Copy)]
pub struct FmPatch {
    pub ops: [FmOpParams; FM_OP_COUNT],
    pub matrix: [[f32; FM_OP_COUNT]; FM_OP_COUNT],
}

impl Default for FmPatch {
    fn default() -> Self {
        fm_recipe_patch(0)
    }
}

fn op(ratio: f32, audio: f32, fold: f32, env: f32, sustain: f32) -> FmOpParams {
    FmOpParams {
        ratio,
        audio,
        fold,
        env,
        sustain: sustain.clamp(0.0, 1.0),
    }
}

fn quiet(ratio: f32) -> FmOpParams {
    op(ratio, 0.0, 0.0, 0.3, 0.0)
}

/// Starting graphs. Each recipe is a different wiring, not a rerun of A→D.
pub fn fm_recipe_patch(index: usize) -> FmPatch {
    let mut p = FmPatch {
        ops: [FmOpParams::default(); FM_OP_COUNT],
        matrix: [[0.0; FM_OP_COUNT]; FM_OP_COUNT],
    };
    match index % FM_RECIPE_COUNT {
        0 => {
            // Bell: two inharmonic strikes into a ringing D.
            p.ops = [
                op(0.52, 0.0, 0.06, 0.10, 0.0),
                op(0.82, 0.0, 0.0, 0.06, 0.0),
                quiet(0.15),
                op(0.12, 1.0, 0.0, 0.50, 0.48),
            ];
            p.matrix[0][3] = 0.70;
            p.matrix[1][3] = 0.38;
        }
        1 => {
            // E.piano: 14:1 tine dies while held; B is a parallel warm sine.
            p.ops = [
                op(0.92, 0.0, 0.0, 0.03, 0.0),
                op(0.12, 0.48, 0.0, 0.32, 0.90),
                quiet(0.15),
                op(0.12, 0.72, 0.0, 0.42, 0.78),
            ];
            p.matrix[0][3] = 0.40;
        }
        2 => {
            // Bass: folded A with feedback, 2:1 into D.
            p.ops = [
                op(0.32, 0.0, 0.40, 0.22, 0.92),
                quiet(0.15),
                quiet(0.15),
                op(0.12, 1.0, 0.14, 0.30, 1.0),
            ];
            p.matrix[0][0] = 0.48;
            p.matrix[0][3] = 0.52;
        }
        3 => {
            // Brass: slow stack A→B→D, no fold — brightness after the press.
            p.ops = [
                op(0.12, 0.0, 0.0, 0.78, 1.0),
                op(0.12, 0.0, 0.0, 0.55, 1.0),
                quiet(0.15),
                op(0.12, 1.0, 0.0, 0.36, 1.0),
            ];
            p.matrix[0][1] = 0.62;
            p.matrix[1][3] = 0.50;
        }
        4 => {
            // Flute: almost just D, tiny A puff + D feedback for air.
            p.ops = [
                op(0.12, 0.0, 0.0, 0.42, 0.25),
                quiet(0.15),
                quiet(0.15),
                op(0.12, 1.0, 0.0, 0.48, 1.0),
            ];
            p.matrix[0][3] = 0.10;
            p.matrix[3][3] = 0.16;
        }
        5 => {
            // Organ: additive drawbars. Instant, holds, almost no FM.
            p.ops = [
                op(0.32, 0.52, 0.0, 0.02, 1.0),
                op(0.42, 0.36, 0.0, 0.02, 1.0),
                op(0.62, 0.20, 0.0, 0.02, 1.0),
                op(0.12, 0.62, 0.0, 0.02, 1.0),
            ];
        }
        6 => {
            // Pluck: A→B→D, all percussive.
            p.ops = [
                op(0.42, 0.0, 0.18, 0.0, 0.0),
                op(0.32, 0.0, 0.0, 0.04, 0.0),
                quiet(0.15),
                op(0.12, 1.0, 0.0, 0.10, 0.0),
            ];
            p.matrix[0][1] = 0.50;
            p.matrix[1][3] = 0.56;
        }
        _ => {
            // Growl: A feedback + A→D + B→D, lots of fold.
            p.ops = [
                op(0.12, 0.12, 0.66, 0.28, 0.85),
                op(0.32, 0.0, 0.28, 0.24, 0.80),
                quiet(0.15),
                op(0.12, 1.0, 0.20, 0.40, 0.90),
            ];
            p.matrix[0][0] = 0.72;
            p.matrix[0][3] = 0.50;
            p.matrix[1][3] = 0.34;
        }
    }
    p
}

#[derive(Debug, Clone, Copy)]
struct FmVoice {
    active: bool,
    channel: u8,
    note: u8,
    phase: [f32; FM_OP_COUNT],
    amp: [f32; FM_OP_COUNT],
    peaked: [bool; FM_OP_COUNT],
    target: f32,
    last: [f32; FM_OP_COUNT],
    releasing: bool,
    age: u64,
}

impl FmVoice {
    const fn silent() -> Self {
        Self {
            active: false,
            channel: 0,
            note: 0,
            phase: [0.0; FM_OP_COUNT],
            amp: [0.0; FM_OP_COUNT],
            peaked: [false; FM_OP_COUNT],
            target: 0.0,
            last: [0.0; FM_OP_COUNT],
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
    selected: usize,
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
        Self {
            sine,
            voices: [FmVoice::silent(); MAX_FM_VOICES],
            serial: 0,
            recipe: 0,
            selected: 3,
            patch: fm_recipe_patch(0),
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

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_recipe(&mut self, index: usize) {
        let index = index % FM_RECIPE_COUNT;
        self.recipe = index;
        self.patch = fm_recipe_patch(index);
        self.selected = 3;
    }

    pub fn set_selected(&mut self, op: usize) {
        self.selected = op % FM_OP_COUNT;
    }

    pub fn set_op_ratio(&mut self, unit: f32) {
        self.patch.ops[self.selected].ratio = unit.clamp(0.0, 1.0);
    }

    pub fn set_op_audio(&mut self, unit: f32) {
        self.patch.ops[self.selected].audio = unit.clamp(0.0, 1.0);
    }

    pub fn set_op_fold(&mut self, unit: f32) {
        self.patch.ops[self.selected].fold = unit.clamp(0.0, 1.0);
    }

    pub fn set_op_env(&mut self, unit: f32) {
        self.patch.ops[self.selected].env = unit.clamp(0.0, 1.0);
    }

    /// MIDI knobs: bright=fold, clang=ratio, hit=env, tail=audio of the selected op.
    pub fn set_bright(&mut self, unit: f32) {
        self.set_op_fold(unit);
    }
    pub fn set_clang(&mut self, unit: f32) {
        self.set_op_ratio(unit);
    }
    pub fn set_hit(&mut self, unit: f32) {
        self.set_op_env(unit);
    }
    pub fn set_tail(&mut self, unit: f32) {
        self.set_op_audio(unit);
    }

    pub fn set_link(&mut self, from: usize, to: usize, amount: f32) {
        let from = from % FM_OP_COUNT;
        let to = to % FM_OP_COUNT;
        self.patch.matrix[from][to] = amount.clamp(0.0, 1.0);
    }

    pub fn clear_links(&mut self) {
        self.patch.matrix = [[0.0; FM_OP_COUNT]; FM_OP_COUNT];
    }

    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(channel, note);
            return;
        }
        self.serial = self.serial.wrapping_add(1);
        let vel = (velocity as f32 / 127.0).clamp(0.05, 1.0);
        let target = vel * VOICE_AMP;

        if let Some(slot) = self.find_playing(channel, note) {
            let v = &mut self.voices[slot];
            v.phase = [0.0; FM_OP_COUNT];
            v.amp = [0.0; FM_OP_COUNT];
            v.peaked = [false; FM_OP_COUNT];
            v.last = [0.0; FM_OP_COUNT];
            v.target = target;
            v.releasing = false;
            v.age = self.serial;
            return;
        }

        let slot = self.free_slot().unwrap_or_else(|| self.steal_slot());
        self.voices[slot] = FmVoice {
            active: true,
            channel,
            note,
            phase: [0.0; FM_OP_COUNT],
            amp: [0.0; FM_OP_COUNT],
            peaked: [false; FM_OP_COUNT],
            target,
            last: [0.0; FM_OP_COUNT],
            releasing: false,
            age: self.serial,
        };
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        if let Some(slot) = self.find_playing(channel, note) {
            let v = &mut self.voices[slot];
            v.releasing = true;
            v.target = 0.0;
        }
    }

    pub fn all_notes_off(&mut self) {
        for v in self.voices.iter_mut() {
            if v.active {
                v.releasing = true;
                v.target = 0.0;
            }
        }
    }

    pub fn silence(&mut self) {
        self.voices = [FmVoice::silent(); MAX_FM_VOICES];
    }

    pub fn render(&mut self, out: &mut [f32], sample_rate: f32, pitch_mul: f32) {
        let sr = sample_rate.max(8000.0);
        let patch = self.patch;
        let mut atk = [0.0f32; FM_OP_COUNT];
        let mut dec = [0.0f32; FM_OP_COUNT];
        let mut rel = [0.0f32; FM_OP_COUNT];
        let mut sus = [0.0f32; FM_OP_COUNT];
        let mut inc_ratio = [0.0f32; FM_OP_COUNT];
        for i in 0..FM_OP_COUNT {
            atk[i] = env_step(patch.ops[i].attack_sec(), sr);
            dec[i] = env_step(patch.ops[i].decay_sec(), sr);
            rel[i] = env_step(patch.ops[i].release_sec(), sr);
            sus[i] = patch.ops[i].sustain.clamp(0.0, 1.0);
            inc_ratio[i] = clang_ratio(patch.ops[i].ratio);
        }
        let size = SINE_SIZE as f32;

        for v in self.voices.iter_mut() {
            if !v.active {
                continue;
            }
            let hz = midi_to_hz(v.note) * pitch_mul.max(0.01) as f64;
            let mut inc = [0.0f32; FM_OP_COUNT];
            for i in 0..FM_OP_COUNT {
                inc[i] = (hz * inc_ratio[i] as f64 * SINE_SIZE as f64 / sr as f64) as f32;
            }

            for sample in out.iter_mut() {
                let mut mix = 0.0f32;
                let prev = v.last;
                for i in 0..FM_OP_COUNT {
                    v.amp[i] = step_env(
                        v.amp[i],
                        v.target,
                        sus[i],
                        v.releasing,
                        &mut v.peaked[i],
                        atk[i],
                        dec[i],
                        rel[i],
                    );
                    let mut mod_phase = 0.0f32;
                    for src in 0..FM_OP_COUNT {
                        let amt = patch.matrix[src][i];
                        if amt > 0.001 {
                            mod_phase += prev[src] * amt * INDEX_SCALE;
                        }
                    }
                    let s = sine_at(&self.sine, v.phase[i] + mod_phase);
                    let shaped = waveshape(s, patch.ops[i].fold) * v.amp[i];
                    v.last[i] = shaped;
                    mix += shaped * patch.ops[i].audio;
                    v.phase[i] += inc[i];
                    if v.phase[i] >= size {
                        v.phase[i] -= size * (v.phase[i] / size).floor();
                    }
                }
                *sample += mix * 0.72;
            }

            if v.releasing && v.amp.iter().all(|a| *a < 0.0008) {
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
        let mut sine = [0.0f32; SINE_SIZE];
        let tau = std::f32::consts::TAU;
        for (i, sample) in sine.iter_mut().enumerate() {
            *sample = (i as f32 * tau / SINE_SIZE as f32).sin();
        }
        let mut phase = [0.0f32; FM_OP_COUNT];
        let mut last = [0.0f32; FM_OP_COUNT];
        let size = SINE_SIZE as f32;
        for sample in out.iter_mut() {
            let mut mix = 0.0f32;
            let prev = last;
            for i in 0..FM_OP_COUNT {
                let inc = clang_ratio(patch.ops[i].ratio) * SINE_SIZE as f32 / n;
                let mut mod_phase = 0.0f32;
                for src in 0..FM_OP_COUNT {
                    let amt = patch.matrix[src][i];
                    if amt > 0.001 {
                        mod_phase += prev[src] * amt * INDEX_SCALE;
                    }
                }
                let s = sine_at(&sine, phase[i] + mod_phase);
                let shaped = waveshape(s, patch.ops[i].fold);
                last[i] = shaped;
                mix += shaped * patch.ops[i].audio;
                phase[i] += inc;
                if phase[i] >= size {
                    phase[i] -= size * (phase[i] / size).floor();
                }
            }
            *sample = mix * 0.72;
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
            let loud = v.amp.iter().copied().fold(0.0f32, f32::max);
            let key = (!v.releasing, loud, v.age);
            if key < best_key {
                best_key = key;
                best = i;
            }
        }
        best
    }

    #[cfg(test)]
    fn test_op_amps(&self) -> Option<[f32; FM_OP_COUNT]> {
        self.voices.iter().find(|v| v.active).map(|v| v.amp)
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

/// Sine → wavefold → pushed square. FIRST LOVE's operator shaper, cheaply.
#[inline]
fn waveshape(s: f32, fold: f32) -> f32 {
    let fold = fold.clamp(0.0, 1.0);
    if fold < 0.01 {
        return s;
    }
    let folded = (s * (1.0 + fold * 3.4)).sin();
    let driven = (s * (1.0 + fold * 7.0)).tanh();
    folded * (1.0 - 0.5 * fold) + driven * (0.5 * fold)
}

#[inline]
fn env_step(seconds: f32, sample_rate: f32) -> f32 {
    1.0 / (seconds.max(0.001) * sample_rate).max(1.0)
}

#[inline]
fn step_env(
    amp: f32,
    peak: f32,
    sustain: f32,
    releasing: bool,
    peaked: &mut bool,
    attack: f32,
    decay: f32,
    release: f32,
) -> f32 {
    if releasing {
        *peaked = false;
        return (amp - release * amp.max(0.02)).max(0.0);
    }
    if !*peaked {
        let next = (amp + attack * peak.max(0.05)).min(peak);
        if next >= peak * 0.98 {
            *peaked = true;
        }
        return next;
    }
    let sus = peak * sustain.clamp(0.0, 1.0);
    if amp > sus {
        (amp - decay * amp.max(0.02)).max(sus)
    } else if amp < sus {
        (amp + attack * sus.max(0.05)).min(sus)
    } else {
        amp
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
        assert_eq!(clang_ratio(0.25), 1.414);
        assert_eq!(clang_ratio(0.35), 2.0);
        assert_eq!(clang_ratio(0.55), 3.5);
        assert_eq!(clang_ratio(1.0), 14.0);
        assert_eq!(clang_label(0.25), "1.41");
        assert_eq!(clang_label(0.35), "2:1");
        assert_ne!(clang_label(0.25), clang_label(0.35));
    }

    #[test]
    fn clang_labels_fit_the_ascii_atlas() {
        let mut seen = std::collections::BTreeSet::new();
        for (i, label) in CLANG_LABELS.iter().enumerate() {
            assert!(
                !label.is_empty() && label.chars().all(|c| (32..96).contains(&(c as u32))),
                "CLANG_LABELS[{i}]={label:?} must stay in ASCII 32..95"
            );
            assert!(
                seen.insert(*label),
                "CLANG_LABELS[{i}]={label:?} is a duplicate"
            );
        }
        assert_eq!(seen.len(), CLANG_RATIOS.len());
    }

    #[test]
    fn preview_cycle_is_nonzero_for_bright_patch() {
        let patch = fm_recipe_patch(0);
        let mut cycle = [0.0f32; 128];
        FmSynth::preview_cycle(patch, &mut cycle);
        let peak = cycle.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.15, "preview peak={peak}");
    }

    #[test]
    fn polyphony_is_capped() {
        let mut fm = FmSynth::new();
        for n in 0..MAX_FM_VOICES + 4 {
            fm.note_on(0, 48 + n as u8, 100);
        }
        assert_eq!(fm.active_count(), MAX_FM_VOICES);
    }

    #[test]
    fn drawing_a_link_packs_and_changes_the_graph() {
        let (from, to, amt) = unpack_fm_link(pack_fm_link(0, 3, 0.7));
        assert_eq!(from, 0);
        assert_eq!(to, 3);
        assert!((amt - 0.7).abs() < 0.02);

        let mut fm = FmSynth::new();
        fm.clear_links();
        fm.set_link(0, 3, 0.8);
        assert!((fm.patch().matrix[0][3] - 0.8).abs() < 1e-5);
        fm.clear_links();
        assert_eq!(fm.patch().matrix[0][3], 0.0);
    }

    #[test]
    fn growl_self_link_is_feedback() {
        let p = fm_recipe_patch(7);
        assert!(p.matrix[0][0] > 0.5);
        assert!(p.ops[0].fold > 0.4);
        assert!(p.matrix[1][3] > 0.2);
    }

    #[test]
    fn recipes_are_not_the_same_graph() {
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..FM_RECIPE_COUNT {
            let p = fm_recipe_patch(i);
            let mut key = [0u8; 16];
            for src in 0..FM_OP_COUNT {
                for dst in 0..FM_OP_COUNT {
                    key[src * FM_OP_COUNT + dst] = (p.matrix[src][dst] * 25.0).round() as u8;
                }
            }
            assert!(
                seen.insert(key),
                "recipe {} reuses another program's wiring",
                fm_recipe(i).id
            );
        }
        assert_eq!(seen.len(), FM_RECIPE_COUNT);
        let brass = fm_recipe_patch(3);
        let bass = fm_recipe_patch(2);
        assert!(
            bass.matrix[0][0] > 0.3 && brass.matrix[0][1] > 0.3,
            "bass is A feedback, brass is A→B→D"
        );
        assert!(fm_recipe_patch(5).ops.iter().all(|o| o.audio > 0.1));
        assert_eq!(fm_recipe_patch(5).matrix, [[0.0; FM_OP_COUNT]; FM_OP_COUNT]);
    }

    #[test]
    fn electric_piano_tine_dies_while_the_key_is_held() {
        let mut fm = FmSynth::new();
        fm.set_recipe(1);
        fm.note_on(0, 60, 120);
        let mut buf = vec![0.0f32; 256];
        fm.render(&mut buf, 48_000.0, 1.0);
        let early = fm.test_op_amps().unwrap();
        assert!(
            early[0] > 0.05,
            "tine should speak on the attack, amp={}",
            early[0]
        );

        for _ in 0..40 {
            buf.iter_mut().for_each(|s| *s = 0.0);
            fm.render(&mut buf, 48_000.0, 1.0);
        }
        let late = fm.test_op_amps().unwrap();
        assert!(
            late[0] < 0.02,
            "14:1 tine must die while held, amp={}",
            late[0]
        );
        assert!(late[1] > 0.08, "warm body B should still be held");
        assert!(late[3] > 0.08, "D body should still be held");
        assert_eq!(fm.active_count(), 1);
    }
}
