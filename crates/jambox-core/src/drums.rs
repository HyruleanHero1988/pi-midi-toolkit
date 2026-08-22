//! Procedural drum voices (Synsonics / TR-ish), one FX insert per model.
//!
//! Most voices are envelopes + one oscillator + noise so a 16-pad kit stays
//! inside the Pi 2 budget. Cowbell is the exception: two square partials (the
//! analog 540/800 Hz clang). Envelopes advance by a per-block multiplier rather
//! than calling `exp` per sample.

/// Number of distinct drum models in the kit.
pub const DRUM_MODEL_COUNT: usize = 16;
/// Simultaneous drum hits (full MPK Bank A+B).
pub const MAX_DRUM_HITS: usize = 16;

/// Kit voices, in MPK factory pad order (notes 36–51).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrumModel {
    Kick,
    Snare,
    Clap,
    HatClosed,
    HatOpen,
    TomLo,
    TomMid,
    Rim,
    KickTight,
    Rimshot,
    Shaker,
    HatPedal,
    TomHi,
    Cowbell,
    Clave,
    Ride,
}

impl DrumModel {
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Self {
        use DrumModel::*;
        const ALL: [DrumModel; DRUM_MODEL_COUNT] = [
            Kick, Snare, Clap, HatClosed, HatOpen, TomLo, TomMid, Rim, KickTight, Rimshot, Shaker,
            HatPedal, TomHi, Cowbell, Clave, Ride,
        ];
        ALL[index % DRUM_MODEL_COUNT]
    }

    pub fn name(self) -> &'static str {
        use DrumModel::*;
        match self {
            Kick => "kick",
            Snare => "snare",
            Clap => "clap",
            HatClosed => "hat_closed",
            HatOpen => "hat_open",
            TomLo => "tom_lo",
            TomMid => "tom_mid",
            Rim => "rim",
            KickTight => "kick_tight",
            Rimshot => "rimshot",
            Shaker => "shaker",
            HatPedal => "hat_pedal",
            TomHi => "tom_hi",
            Cowbell => "cowbell",
            Clave => "clave",
            Ride => "ride",
        }
    }
}

/// MPK factory MPC program: notes 36–51 map to the 16 kit voices in order.
pub fn drum_model_for_note(note: u8) -> DrumModel {
    let idx = note.wrapping_sub(36) as usize;
    DrumModel::from_index(idx % DRUM_MODEL_COUNT)
}

/// Live kit macros, all 0..1.
#[derive(Debug, Clone, Copy)]
pub struct DrumMacros {
    pub pitch: f32,
    pub decay: f32,
    pub noise: f32,
    pub tone: f32,
}

impl Default for DrumMacros {
    fn default() -> Self {
        Self {
            pitch: 0.45,
            decay: 0.40,
            noise: 0.55,
            tone: 0.60,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Hit {
    active: bool,
    model: DrumModel,
    velocity: f32,
    phase: f64,
    /// Second oscillator (cowbell clang).
    phase2: f64,
    /// Body / tonal envelope level.
    body_env: f32,
    /// Noise envelope level.
    noise_env: f32,
    /// Transient click envelope.
    click_env: f32,
    body_tau: f32,
    noise_tau: f32,
    /// Frequency sweep state for pitched drums.
    freq: f32,
    freq_end: f32,
    freq_tau: f32,
    /// Independent second partial (cowbell 800 Hz, unused elsewhere).
    freq2: f32,
    noise_lp: f32,
    /// Cowbell presence filter — must not share `noise_lp` with the noise tone LP.
    color_lp: f32,
    /// Clap bandpass (upper pole). Cowbell unused.
    bp_lp: f32,
    elapsed: f32,
    body_amp: f32,
    age: u64,
}

impl Hit {
    const fn silent() -> Self {
        Self {
            active: false,
            model: DrumModel::Kick,
            velocity: 0.0,
            phase: 0.0,
            phase2: 0.0,
            body_env: 0.0,
            noise_env: 0.0,
            click_env: 0.0,
            body_tau: 0.1,
            noise_tau: 0.1,
            freq: 50.0,
            freq_end: 40.0,
            freq_tau: 0.02,
            freq2: 0.0,
            noise_lp: 0.0,
            color_lp: 0.0,
            bp_lp: 0.0,
            elapsed: 0.0,
            body_amp: 0.38,
            age: 0,
        }
    }
}

/// Drum voice allocator + renderer. One instance owns all 16 models.
pub struct DrumKit {
    hits: [Hit; MAX_DRUM_HITS],
    macros: DrumMacros,
    rng: u32,
    serial: u64,
    sample_rate: f32,
}

impl DrumKit {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            hits: [Hit::silent(); MAX_DRUM_HITS],
            macros: DrumMacros::default(),
            rng: 0x1234_5678,
            serial: 0,
            sample_rate: sample_rate.max(8000.0),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(8000.0);
    }

    pub fn macros(&self) -> DrumMacros {
        self.macros
    }

    pub fn set_macros(&mut self, macros: DrumMacros) {
        self.macros = DrumMacros {
            pitch: macros.pitch.clamp(0.0, 1.0),
            decay: macros.decay.clamp(0.0, 1.0),
            noise: macros.noise.clamp(0.0, 1.0),
            tone: macros.tone.clamp(0.0, 1.0),
        };
    }

    pub fn active_count(&self) -> usize {
        self.hits.iter().filter(|h| h.active).count()
    }

    pub fn silence(&mut self) {
        self.hits = [Hit::silent(); MAX_DRUM_HITS];
    }

    /// Trigger a pad. Voice-steals the oldest hit when the kit is full.
    pub fn trigger(&mut self, model: DrumModel, velocity: u8) {
        self.serial = self.serial.wrapping_add(1);
        let slot = self
            .hits
            .iter()
            .position(|h| !h.active)
            .unwrap_or_else(|| self.oldest_slot());

        let m = self.macros;
        let vel = (velocity as f32 / 127.0).clamp(0.05, 1.0);
        let mut hit = Hit {
            active: true,
            model,
            velocity: vel,
            phase: 0.0,
            body_env: 1.0,
            noise_env: 1.0,
            click_env: 1.0,
            age: self.serial,
            ..Hit::silent()
        };

        use DrumModel::*;
        match model {
            Kick | KickTight | TomLo | TomMid | TomHi => {
                let (base, end_lo, end_span, drop, body) = match model {
                    Kick => (
                        50.0,
                        28.0,
                        16.0,
                        0.016 + 0.05 * (1.0 - m.decay),
                        0.07 + 0.40 * m.decay,
                    ),
                    KickTight => (
                        68.0,
                        40.0,
                        20.0,
                        0.010 + 0.03 * (1.0 - m.decay),
                        0.035 + 0.18 * m.decay,
                    ),
                    TomLo => (
                        85.0,
                        55.0,
                        25.0,
                        0.025 + 0.07 * (1.0 - m.decay),
                        0.08 + 0.38 * m.decay,
                    ),
                    TomMid => (
                        120.0,
                        75.0,
                        30.0,
                        0.022 + 0.06 * (1.0 - m.decay),
                        0.06 + 0.30 * m.decay,
                    ),
                    _ => (
                        170.0,
                        100.0,
                        40.0,
                        0.018 + 0.05 * (1.0 - m.decay),
                        0.045 + 0.22 * m.decay,
                    ),
                };
                hit.freq = base
                    * 2f32.powf((m.pitch - 0.5) * if matches!(model, KickTight) { 1.6 } else { 1.8 });
                hit.freq_end = end_lo + end_span * m.pitch;
                hit.freq_tau = drop;
                hit.body_tau = body;
                hit.noise_tau = body;
                hit.body_amp = match model {
                    Kick => 0.38,
                    KickTight => 0.34,
                    TomLo => 0.32,
                    TomMid => 0.30,
                    _ => 0.28,
                };
            }
            Snare | Rimshot => {
                let f0 = if model == Rimshot { 200.0 } else { 175.0 };
                hit.freq = f0 * 2f32.powf((m.pitch - 0.5) * 1.4);
                hit.freq_end = hit.freq;
                hit.freq_tau = 1.0;
                hit.body_tau = if model == Rimshot {
                    0.018 + 0.10 * m.decay
                } else {
                    0.03 + 0.18 * m.decay
                };
                hit.noise_tau = if model == Rimshot {
                    0.025 + 0.14 * m.decay
                } else {
                    0.04 + 0.28 * m.decay
                };
            }
            Clap => {
                // 808 clap: four short slaps through a ~1 kHz bandpass, then a
                // delayed quieter wash. Not a snare (no tonal body, no wire from t=0).
                hit.freq = 700.0 * 2f32.powf((m.pitch - 0.5) * 1.0);
                hit.freq_end = 2600.0 * 2f32.powf((m.pitch - 0.5) * 0.6 + (m.tone - 0.5) * 0.9);
                hit.body_tau = 0.0018; // slap width
                hit.noise_tau = 0.045 + 0.14 * m.decay; // verb after the slaps
            }
            HatClosed | HatOpen | HatPedal | Shaker => {
                hit.body_tau = 0.01;
                hit.noise_tau = match model {
                    HatOpen => 0.05 + 0.40 * m.decay,
                    HatPedal => 0.008 + 0.04 * m.decay,
                    Shaker => 0.02 + 0.10 * m.decay,
                    _ => 0.015 + 0.08 * m.decay,
                };
                hit.freq = 40.0 + 80.0 * m.pitch; // shaker grain rate
            }
            Ride => {
                // Metallic ding + wash so it isn't just a long closed hat.
                hit.freq = 3200.0 * 2f32.powf((m.pitch - 0.5) * 0.45);
                hit.freq_end = hit.freq;
                hit.freq_tau = 1.0;
                hit.body_tau = 0.08 + 0.32 * m.decay;
                hit.noise_tau = 0.12 + 0.55 * m.decay;
                hit.body_amp = 0.09;
            }
            Rim => {
                hit.freq = 520.0 * 2f32.powf((m.pitch - 0.5) * 1.2);
                hit.freq_end = hit.freq;
                hit.freq_tau = 1.0;
                hit.body_tau = 0.012 + 0.05 * m.decay;
                hit.noise_tau = 0.004;
                hit.body_amp = 0.22;
            }
            Clave => {
                // Woody tick: high, short, with a stick click. Not a tiny cowbell.
                hit.freq = 2450.0 * 2f32.powf((m.pitch - 0.5) * 0.55);
                hit.freq_end = hit.freq;
                hit.freq_tau = 1.0;
                hit.body_tau = 0.006 + 0.016 * m.decay;
                hit.noise_tau = 0.003;
                hit.body_amp = 0.22;
            }
            Cowbell => {
                // TR-808: two inharmonic square partials (~540 Hz + ~800 Hz).
                hit.freq = 540.0 * 2f32.powf((m.pitch - 0.5) * 1.0);
                hit.freq2 = 800.0 * 2f32.powf((m.pitch - 0.5) * 1.0);
                hit.freq_end = hit.freq;
                hit.freq_tau = 1.0;
                hit.body_tau = 0.08 + 0.32 * m.decay;
                hit.noise_tau = 0.012;
                hit.body_amp = 0.13;
            }
        }
        self.hits[slot] = hit;
    }

    fn oldest_slot(&self) -> usize {
        let mut best = 0;
        let mut best_age = u64::MAX;
        for (i, h) in self.hits.iter().enumerate() {
            if h.age < best_age {
                best_age = h.age;
                best = i;
            }
        }
        best
    }

    /// Distinct models currently sounding. Returns the count written to `out`.
    pub fn active_models(&self, out: &mut [usize; MAX_DRUM_HITS]) -> usize {
        let mut n = 0;
        for h in self.hits.iter().filter(|h| h.active) {
            let idx = h.model.index();
            if !out[..n].contains(&idx) {
                out[n] = idx;
                n += 1;
            }
        }
        n
    }

    /// Sum every active hit of `model` into `out` (additive). Allocation-free.
    pub fn render_model(&mut self, model: DrumModel, out: &mut [f32]) {
        let sr = self.sample_rate;
        let m = self.macros;
        let tone = m.tone;
        let noise_amt = m.noise;
        // Noise low-pass coefficient: darker as tone drops.
        let lp_coef = (0.05 + 0.95 * tone).clamp(0.02, 1.0);

        for idx in 0..MAX_DRUM_HITS {
            if !self.hits[idx].active || self.hits[idx].model != model {
                continue;
            }
            let mut hit = self.hits[idx];
            let body_coef = decay_coef(hit.body_tau, sr);
            let noise_coef = decay_coef(hit.noise_tau, sr);
            let click_coef = decay_coef(0.0035, sr);
            let freq_coef = decay_coef(hit.freq_tau, sr);
            let vel = hit.velocity;
            let clap_hp_coef = (std::f32::consts::TAU * hit.freq / sr).clamp(0.03, 0.45);
            let clap_lp_coef = (std::f32::consts::TAU * hit.freq_end / sr).clamp(0.10, 0.85);

            for sample in out.iter_mut() {
                let white = self.next_noise();
                hit.noise_lp += (white - hit.noise_lp) * lp_coef;
                let noise = hit.noise_lp;

                let value = match hit.model {
                    DrumModel::Kick
                    | DrumModel::KickTight
                    | DrumModel::TomLo
                    | DrumModel::TomMid
                    | DrumModel::TomHi => {
                        hit.freq += (hit.freq_end - hit.freq) * freq_coef;
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        let body = (hit.phase.sin() as f32) * hit.body_env * hit.body_amp * vel;
                        let click = hit.click_env * 0.18 * vel * white;
                        body + click + noise * 0.05 * noise_amt * vel * hit.body_env
                    }
                    DrumModel::Snare | DrumModel::Rimshot => {
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        let body_amp = if hit.model == DrumModel::Rimshot {
                            0.10
                        } else {
                            0.16
                        };
                        let noise_amp = if hit.model == DrumModel::Rimshot {
                            0.28 + 0.45 * noise_amt
                        } else {
                            0.18 + 0.40 * noise_amt
                        };
                        let click_amp = if hit.model == DrumModel::Rimshot {
                            0.25
                        } else {
                            0.12
                        };
                        let body = (hit.phase.sin() as f32) * hit.body_env * body_amp * vel;
                        body + noise * hit.noise_env * noise_amp * vel
                            + hit.click_env * click_amp * vel * white
                    }
                    DrumModel::Clap => {
                        // Bandpass ~0.7–2.6 kHz (palms), not the kit tone LP (snare wire).
                        hit.color_lp += (white - hit.color_lp) * clap_hp_coef;
                        let hp = white - hit.color_lp;
                        hit.bp_lp += (hp - hit.bp_lp) * clap_lp_coef;
                        let t = hit.elapsed;
                        let slap_tau = hit.body_tau;
                        let slaps = clap_slap(t, 0.000, slap_tau)
                            + 0.82 * clap_slap(t, 0.010, slap_tau * 1.10)
                            + 0.64 * clap_slap(t, 0.019, slap_tau * 1.20)
                            + 0.48 * clap_slap(t, 0.029, slap_tau * 1.30);
                        let verb = clap_slap(t, 0.024, hit.noise_tau) * (0.20 + 0.18 * noise_amt);
                        hit.bp_lp * (slaps * 0.70 + verb) * vel * 0.85
                    }
                    DrumModel::HatClosed
                    | DrumModel::HatOpen
                    | DrumModel::HatPedal
                    | DrumModel::Shaker => {
                        let bright = white - noise * 0.85;
                        let amp = match hit.model {
                            DrumModel::HatOpen => 0.14 + 0.30 * noise_amt,
                            DrumModel::HatPedal => 0.12 + 0.22 * noise_amt,
                            DrumModel::Shaker => 0.12 + 0.28 * noise_amt,
                            _ => 0.14 + 0.30 * noise_amt,
                        };
                        let grain = if hit.model == DrumModel::Shaker {
                            hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                            0.55 + 0.45 * hit.phase.sin() as f32
                        } else {
                            1.0
                        };
                        bright * hit.noise_env * grain * amp * vel * (0.35 + 0.65 * tone)
                            + noise * hit.noise_env * 0.10 * (1.0 - tone) * vel
                    }
                    DrumModel::Ride => {
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        let ding = (hit.phase.sin() as f32) * hit.body_env * hit.body_amp * vel;
                        let bright = white - noise * 0.85;
                        let wash = (0.10 + 0.22 * noise_amt) * vel;
                        ding + bright * hit.noise_env * wash * (0.35 + 0.65 * tone)
                            + noise * hit.noise_env * 0.10 * (1.0 - tone) * vel
                    }
                    DrumModel::Rim => {
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        (hit.phase.sin() as f32) * hit.body_env * hit.body_amp * vel
                            + hit.click_env * 0.18 * vel * white
                    }
                    DrumModel::Clave => {
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        (hit.phase.sin() as f32) * hit.body_env * hit.body_amp * vel
                            + hit.click_env * 0.22 * vel * white
                    }
                    DrumModel::Cowbell => {
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        hit.phase2 += (hit.freq2 as f64 / sr as f64) * std::f64::consts::TAU;
                        let mix = cheap_square(hit.phase) + 0.72 * cheap_square(hit.phase2);
                        // Cheap presence: take out a bit of the fundamental thump.
                        hit.color_lp += (mix - hit.color_lp) * 0.16;
                        let colored = mix - 0.55 * hit.color_lp;
                        colored * hit.body_env * hit.body_amp * vel
                            + noise * hit.body_env * 0.03 * noise_amt * vel
                    }
                };

                *sample += value;

                hit.body_env *= body_coef;
                hit.noise_env *= noise_coef;
                hit.click_env *= click_coef;
                hit.elapsed += 1.0 / sr;
            }

            if hit.phase.abs() > 1e9 {
                hit.phase %= std::f64::consts::TAU;
            }
            if hit.phase2.abs() > 1e9 {
                hit.phase2 %= std::f64::consts::TAU;
            }
            if hit.body_env < 1e-4 && hit.noise_env < 1e-4 {
                hit.active = false;
            }
            self.hits[idx] = hit;
        }
    }

    /// Deterministic white noise in −1..1 (xorshift; no allocation, no syscall).
    #[inline]
    fn next_noise(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        ((x >> 8) as f32 / 8_388_608.0) - 1.0
    }
}

#[inline]
fn clap_slap(t: f32, offset: f32, tau: f32) -> f32 {
    let d = t - offset;
    if d < 0.0 {
        0.0
    } else {
        (-d / tau.max(0.0004)).exp()
    }
}

#[inline]
fn cheap_square(phase: f64) -> f32 {
    let t = phase.rem_euclid(std::f64::consts::TAU);
    if t < std::f64::consts::PI {
        1.0
    } else {
        -1.0
    }
}

#[inline]
fn decay_coef(tau_sec: f32, sample_rate: f32) -> f32 {
    let n = (tau_sec.max(0.0005) * sample_rate).max(1.0);
    (-1.0 / n).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_pad_notes_map_in_order() {
        assert_eq!(drum_model_for_note(36), DrumModel::Kick);
        assert_eq!(drum_model_for_note(37), DrumModel::Snare);
        assert_eq!(drum_model_for_note(51), DrumModel::Ride);
    }

    #[test]
    fn trigger_makes_sound_then_frees_the_slot() {
        let mut kit = DrumKit::new(48_000.0);
        kit.trigger(DrumModel::Kick, 120);
        assert_eq!(kit.active_count(), 1);

        let mut buf = vec![0.0f32; 512];
        kit.render_model(DrumModel::Kick, &mut buf);
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01, "kick should be audible, peak={peak}");

        for _ in 0..400 {
            buf.iter_mut().for_each(|v| *v = 0.0);
            kit.render_model(DrumModel::Kick, &mut buf);
        }
        assert_eq!(kit.active_count(), 0, "hit should decay and free its slot");
    }

    #[test]
    fn models_render_independently() {
        let mut kit = DrumKit::new(48_000.0);
        kit.trigger(DrumModel::Snare, 120);
        let mut buf = vec![0.0f32; 256];
        kit.render_model(DrumModel::Kick, &mut buf);
        assert!(buf.iter().all(|v| *v == 0.0), "kick bus must stay dry");
        kit.render_model(DrumModel::Snare, &mut buf);
        assert!(buf.iter().any(|v| *v != 0.0));
    }

    #[test]
    fn active_models_lists_each_model_once() {
        let mut kit = DrumKit::new(48_000.0);
        kit.trigger(DrumModel::Kick, 100);
        kit.trigger(DrumModel::Kick, 100);
        kit.trigger(DrumModel::HatClosed, 100);
        let mut out = [0usize; MAX_DRUM_HITS];
        let n = kit.active_models(&mut out);
        assert_eq!(n, 2);
    }

    #[test]
    fn kit_is_voice_capped() {
        let mut kit = DrumKit::new(48_000.0);
        for _ in 0..(MAX_DRUM_HITS + 8) {
            kit.trigger(DrumModel::Clap, 100);
        }
        assert_eq!(kit.active_count(), MAX_DRUM_HITS);
    }

    #[test]
    fn every_model_produces_signal() {
        for i in 0..DRUM_MODEL_COUNT {
            let model = DrumModel::from_index(i);
            let mut kit = DrumKit::new(48_000.0);
            kit.trigger(model, 127);
            let mut buf = vec![0.0f32; 2048];
            kit.render_model(model, &mut buf);
            let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.005, "{} was silent (peak {peak})", model.name());
        }
    }

    fn rms(buf: &[f32], start: usize, end: usize) -> f32 {
        let n = (end.saturating_sub(start)).max(1) as f32;
        (buf[start..end].iter().map(|s| s * s).sum::<f32>() / n).sqrt()
    }

    fn render_note(model: DrumModel, n: usize) -> Vec<f32> {
        let mut kit = DrumKit::new(44_100.0);
        kit.trigger(model, 110);
        let mut buf = vec![0.0f32; n];
        kit.render_model(model, &mut buf);
        buf
    }

    #[test]
    fn cowbell_rings_longer_and_harder_than_clave() {
        // 808 cowbell is two mid squares that hang; clave is a short woody tick.
        let cow = render_note(DrumModel::Cowbell, 12_000);
        let clav = render_note(DrumModel::Clave, 12_000);
        let cow_early = rms(&cow, 0, 256);
        let clav_early = rms(&clav, 0, 256);
        let cow_late = rms(&cow, 4000, 5000); // ~90–113 ms
        let clav_late = rms(&clav, 4000, 5000);
        assert!(
            cow_early > 0.02,
            "cowbell should speak immediately, got {cow_early}"
        );
        assert!(
            clav_early > 0.02,
            "clave should speak immediately, got {clav_early}"
        );
        assert!(
            cow_late > clav_late * 4.0,
            "cowbell should still ring after clave dies (cow {cow_late} vs clav {clav_late})"
        );
        assert!(
            clav_late < 0.002,
            "clave should be gone by 100ms, got {clav_late}"
        );
    }

    #[test]
    fn cowbell_is_not_a_sine_blip() {
        // Two independent squares (540 + 800 Hz) produce more zero crossings than
        // a single mid sine, and enough bite to read as metallic rather than a beep.
        let cow = render_note(DrumModel::Cowbell, 2048);
        let mut crossings = 0usize;
        for w in cow.windows(2) {
            if w[0].signum() != w[1].signum() && w[0] != 0.0 {
                crossings += 1;
            }
        }
        // 540+800 Hz over 2048/44100 ≈ 46ms should cross dozens of times.
        assert!(
            crossings > 40,
            "cowbell should be metallic/square, got {crossings} crossings"
        );
        let peak = cow.iter().copied().map(f32::abs).fold(0.0, f32::max);
        assert!(peak > 0.08, "cowbell should have some bite, peak {peak}");
    }

    fn goertzel_power(buf: &[f32], sr: f32, freq: f32) -> f32 {
        let w = std::f32::consts::TAU * freq / sr;
        let coeff = 2.0 * w.cos();
        let mut s1 = 0.0f32;
        let mut s2 = 0.0f32;
        for &x in buf {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    #[test]
    fn clap_stutters_and_has_no_snare_body() {
        let clap = render_note(DrumModel::Clap, 8_000);
        let snare = render_note(DrumModel::Snare, 8_000);
        let sr = 44_100.0f32;
        // Distinct slaps: energy dips between the first two ~10 ms hits.
        let first = rms(&clap, 0, 90);
        let dip = rms(&clap, 200, 320); // ~4.5–7.3 ms
        let second = rms(&clap, 420, 520); // ~9.5–11.8 ms
        assert!(first > 0.02, "clap should speak, first-slap rms {first}");
        assert!(
            dip < first * 0.55,
            "clap should stutter, dip {dip} vs first {first}"
        );
        assert!(
            second > dip * 1.6,
            "second slap should come back (second {second} vs dip {dip})"
        );

        let clap_body = goertzel_power(&clap[..3500], sr, 175.0);
        let snare_body = goertzel_power(&snare[..3500], sr, 175.0);
        assert!(
            snare_body > clap_body * 4.0,
            "snare keeps a 175 Hz body; clap must not (snare {snare_body} vs clap {clap_body})"
        );
        let clap_mid = goertzel_power(&clap[..3500], sr, 1200.0);
        assert!(
            clap_mid > clap_body,
            "clap energy should sit in the mid band, not the kick/snare body"
        );
    }
}
