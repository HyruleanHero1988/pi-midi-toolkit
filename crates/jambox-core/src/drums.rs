//! Procedural drum voices (Synsonics / TR-ish), one FX insert per model.
//!
//! Everything is envelopes + one oscillator + noise, so a full 16-pad kit stays
//! inside the Pi 2 budget. Envelopes advance by a per-block multiplier rather than
//! calling `exp` per sample.

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
    noise_lp: f32,
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
            body_env: 0.0,
            noise_env: 0.0,
            click_env: 0.0,
            body_tau: 0.1,
            noise_tau: 0.1,
            freq: 50.0,
            freq_end: 40.0,
            freq_tau: 0.02,
            noise_lp: 0.0,
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
                    // Start in the audible thump band (~100 Hz) and fall toward
                    // ~50 Hz. Older 50→28 Hz tuning vanished on many speakers;
                    // only the noise click remained.
                    Kick => (
                        115.0,
                        55.0,
                        22.0,
                        0.028 + 0.065 * (1.0 - m.decay),
                        0.12 + 0.55 * m.decay,
                    ),
                    KickTight => (
                        85.0,
                        48.0,
                        22.0,
                        0.014 + 0.04 * (1.0 - m.decay),
                        0.055 + 0.26 * m.decay,
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
                    Kick => 0.92,
                    KickTight => 0.62,
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
                hit.body_tau = 0.01;
                hit.noise_tau = 0.03 + 0.22 * m.decay;
            }
            HatClosed | HatOpen | HatPedal | Ride | Shaker => {
                hit.body_tau = 0.01;
                hit.noise_tau = match model {
                    HatOpen => 0.05 + 0.40 * m.decay,
                    HatPedal => 0.008 + 0.04 * m.decay,
                    Ride => 0.12 + 0.55 * m.decay,
                    Shaker => 0.02 + 0.10 * m.decay,
                    _ => 0.015 + 0.08 * m.decay,
                };
                hit.freq = 40.0 + 80.0 * m.pitch; // shaker grain rate
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
                hit.freq = 1800.0 * 2f32.powf((m.pitch - 0.5) * 0.8);
                hit.freq_end = hit.freq;
                hit.freq_tau = 1.0;
                hit.body_tau = 0.008 + 0.035 * m.decay;
                hit.noise_tau = 0.004;
                hit.body_amp = 0.20;
            }
            Cowbell => {
                hit.freq = 540.0 * 2f32.powf((m.pitch - 0.5) * 1.0);
                hit.freq_end = 800.0 * 2f32.powf((m.pitch - 0.5) * 1.0);
                hit.freq_tau = 1.0;
                hit.body_tau = 0.05 + 0.28 * m.decay;
                hit.noise_tau = 0.01;
                hit.body_amp = 0.18;
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
                        // Exponential approach: f_end + (f - f_end) * exp(-dt/tau).
                        hit.freq = hit.freq_end + (hit.freq - hit.freq_end) * freq_coef;
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        let raw = (hit.phase.sin() as f32) * hit.body_env * hit.body_amp * vel;
                        // Soft saturate so the sine body reads louder on small speakers.
                        let body = (raw * 1.35).tanh();
                        let (click_amt, noise_scale) = match hit.model {
                            DrumModel::Kick => (0.055, 0.010),
                            DrumModel::KickTight => (0.050, 0.012),
                            _ => (0.14, 0.035),
                        };
                        let click = hit.click_env * click_amt * vel * white;
                        body + click + noise * noise_scale * noise_amt * vel * hit.body_env
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
                        let t = hit.elapsed;
                        let bursts = (-(t * t) / 0.0000008).exp()
                            + (-((t - 0.012) * (t - 0.012)) / 0.0000010).exp()
                            + (-((t - 0.024) * (t - 0.024)) / 0.0000012).exp();
                        noise
                            * (bursts * 0.45 + hit.noise_env * 0.28)
                            * (0.28 + 0.4 * noise_amt)
                            * vel
                    }
                    DrumModel::HatClosed
                    | DrumModel::HatOpen
                    | DrumModel::HatPedal
                    | DrumModel::Ride
                    | DrumModel::Shaker => {
                        let bright = white - noise * 0.85;
                        let amp = match hit.model {
                            DrumModel::HatOpen => 0.14 + 0.30 * noise_amt,
                            DrumModel::HatPedal => 0.12 + 0.22 * noise_amt,
                            DrumModel::Ride => 0.10 + 0.22 * noise_amt,
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
                    DrumModel::Rim => {
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        (hit.phase.sin() as f32) * hit.body_env * hit.body_amp * vel
                            + hit.click_env * 0.18 * vel * white
                    }
                    DrumModel::Clave => {
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        (hit.phase.sin() as f32) * hit.body_env * hit.body_amp * vel
                    }
                    DrumModel::Cowbell => {
                        hit.phase += (hit.freq as f64 / sr as f64) * std::f64::consts::TAU;
                        let p2 = hit.phase * (hit.freq_end as f64 / hit.freq.max(1.0) as f64);
                        ((hit.phase.sin() as f32) + 0.7 * (p2.sin() as f32))
                            * hit.body_env
                            * hit.body_amp
                            * vel
                            + noise * hit.body_env * 0.04 * noise_amt * vel
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

/// Offline one-shot preview for UI scopes (not used on the audio thread).
pub fn preview_drum(
    model: DrumModel,
    macros: DrumMacros,
    sample_rate: f32,
    seconds: f32,
) -> Vec<f32> {
    let sr = sample_rate.max(8000.0);
    let n = ((seconds.clamp(0.05, 2.0) * sr) as usize).max(64);
    let mut kit = DrumKit::new(sr);
    kit.set_macros(macros);
    kit.trigger(model, 120);
    let mut out = vec![0.0f32; n];
    let mut pos = 0;
    while pos < n && kit.active_count() > 0 {
        let end = (pos + 256).min(n);
        let slice = &mut out[pos..end];
        for s in slice.iter_mut() {
            *s = 0.0;
        }
        kit.render_model(model, slice);
        pos = end;
    }
    out
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

    #[test]
    fn kick_body_dominates_click_noise() {
        let mut kit = DrumKit::new(48_000.0);
        kit.set_macros(DrumMacros {
            pitch: 0.5,
            decay: 0.55,
            noise: 0.2,
            tone: 0.55,
        });
        kit.trigger(DrumModel::Kick, 127);
        let mut buf = vec![0.0f32; 2048];
        kit.render_model(DrumModel::Kick, &mut buf);
        let peak = buf.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.25, "kick body should be strong, peak={peak}");
        // Later samples (after click dies) should still carry body energy.
        let late = buf[800..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(late > 0.05, "sustained body after click, late={late}");
    }

    #[test]
    fn kick_pitch_sweep_starts_above_end_and_falls() {
        let mut kit = DrumKit::new(48_000.0);
        kit.trigger(DrumModel::Kick, 127);
        let start = kit.hits.iter().find(|h| h.active).unwrap().freq;
        let end = kit.hits.iter().find(|h| h.active).unwrap().freq_end;
        assert!(
            start > end + 20.0,
            "kick should start well above its floor (start={start}, end={end})"
        );
        assert!(
            start > 80.0,
            "kick start should sit in the audible thump band, got {start}"
        );

        // ~5 ms — should still be closer to start than to end.
        let mut buf = vec![0.0f32; 256];
        kit.render_model(DrumModel::Kick, &mut buf);
        let mid = kit.hits.iter().find(|h| h.active).unwrap().freq;
        assert!(
            mid > end + 10.0,
            "pitch sweep jumped to the floor too fast (mid={mid}, end={end})"
        );
        assert!(mid < start, "pitch should fall over time (mid={mid}, start={start})");
    }
}
