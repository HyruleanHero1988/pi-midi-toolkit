//! Insert FX: drive → flanger → delay → short multi-tap tank.
//!
//! One [`FxUnit`] per wavetable voice, per drum model, per drum group, and one on
//! the master bus (see `PLAN.md` "Effects (insert + kit group + bus)"). Buffers are
//! sized in [`FxUnit::new`]; `process` never allocates.

const DELAY_MAX_SEC: f32 = 1.0;
const REVERB_MAX_SEC: f32 = 0.55;
/// Classic analog flanger window (~1–12 ms). Sized once in `new`.
const FLANGER_MAX_SEC: f32 = 0.020;

/// Plain-old-data FX amounts, all 0..1. Cheap to copy across the command ring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxParams {
    pub drive: f32,
    pub delay_time: f32,
    pub delay_fb: f32,
    pub delay_mix: f32,
    pub reverb_size: f32,
    pub reverb_mix: f32,
    pub flanger_mix: f32,
    pub flanger_rate: f32,
    pub flanger_depth: f32,
    pub flanger_fb: f32,
}

impl Default for FxParams {
    fn default() -> Self {
        Self {
            drive: 0.0,
            delay_time: 0.28,
            delay_fb: 0.35,
            delay_mix: 0.0,
            reverb_size: 0.45,
            reverb_mix: 0.0,
            flanger_mix: 0.0,
            flanger_rate: 0.35,
            flanger_depth: 0.70,
            flanger_fb: 0.45,
        }
    }
}

impl FxParams {
    /// True when the unit would be a no-op, so `process` can be skipped entirely.
    pub fn is_bypassed(&self) -> bool {
        self.drive <= 0.001
            && self.delay_mix <= 0.001
            && self.delay_fb <= 0.001
            && self.reverb_mix <= 0.001
            && self.flanger_mix <= 0.001
    }

    /// True when a wet mix would still be audible after the dry voice dies.
    pub fn is_wet(&self) -> bool {
        self.delay_mix > 0.001 || self.reverb_mix > 0.001 || self.flanger_mix > 0.001
    }
}

/// One FX insert with its own delay line and reverb tank.
pub struct FxUnit {
    params: FxParams,
    sample_rate: f32,
    delay: Vec<f32>,
    delay_pos: usize,
    reverb: Vec<f32>,
    reverb_pos: usize,
    reverb_prev: f32,
    flanger: Vec<f32>,
    flanger_pos: usize,
    flanger_phase: f32,
    /// Samples of silence still worth running so delay / reverb can finish.
    tail_hold: u32,
}

impl FxUnit {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(8000.0);
        let dlen = (sample_rate * DELAY_MAX_SEC) as usize + 64;
        let rlen = (sample_rate * REVERB_MAX_SEC) as usize + 64;
        let flen = (sample_rate * FLANGER_MAX_SEC) as usize + 8;
        Self {
            params: FxParams::default(),
            sample_rate,
            delay: vec![0.0; dlen],
            delay_pos: 0,
            reverb: vec![0.0; rlen],
            reverb_pos: 0,
            reverb_prev: 0.0,
            flanger: vec![0.0; flen.max(32)],
            flanger_pos: 0,
            flanger_phase: 0.0,
            tail_hold: 0,
        }
    }

    pub fn params(&self) -> FxParams {
        self.params
    }

    pub fn set_params(&mut self, params: FxParams) {
        self.params = FxParams {
            drive: params.drive.clamp(0.0, 1.0),
            delay_time: params.delay_time.clamp(0.0, 1.0),
            delay_fb: params.delay_fb.clamp(0.0, 1.0),
            delay_mix: params.delay_mix.clamp(0.0, 1.0),
            reverb_size: params.reverb_size.clamp(0.0, 1.0),
            reverb_mix: params.reverb_mix.clamp(0.0, 1.0),
            flanger_mix: params.flanger_mix.clamp(0.0, 1.0),
            flanger_rate: params.flanger_rate.clamp(0.0, 1.0),
            flanger_depth: params.flanger_depth.clamp(0.0, 1.0),
            flanger_fb: params.flanger_fb.clamp(0.0, 1.0),
        };
    }

    /// Clear tails — used on panic / all-notes-off so a stale echo can't leak.
    pub fn reset(&mut self) {
        self.delay.iter_mut().for_each(|v| *v = 0.0);
        self.reverb.iter_mut().for_each(|v| *v = 0.0);
        self.flanger.iter_mut().for_each(|v| *v = 0.0);
        self.delay_pos = 0;
        self.reverb_pos = 0;
        self.reverb_prev = 0.0;
        self.flanger_pos = 0;
        self.flanger_phase = 0.0;
        self.tail_hold = 0;
    }

    /// Wet tanks still have something to say after the last dry voice went idle.
    pub fn has_tail(&self) -> bool {
        self.tail_hold > 0 && self.params.is_wet()
    }

    /// Process in place. Allocation-free; safe to call from the audio thread.
    pub fn process(&mut self, buf: &mut [f32]) {
        if buf.is_empty() {
            return;
        }
        let p = self.params;
        let input_hot = buf.iter().any(|s| s.abs() > 1e-4);

        if p.drive > 0.001 {
            let amount = 1.0 + p.drive * 12.0;
            let norm = 1.0 / amount.tanh().max(0.25);
            for s in buf.iter_mut() {
                *s = (*s * amount).tanh() * norm;
            }
        }

        if p.flanger_mix > 0.001 {
            let flen = self.flanger.len();
            let rate_hz = 0.08 + p.flanger_rate * 6.0;
            let depth = p.flanger_depth;
            let fb = p.flanger_fb.min(0.92);
            let mix = p.flanger_mix;
            let dry = 1.0 - mix;
            let min_delay = 0.0012 * self.sample_rate;
            let max_extra = 0.0088 * self.sample_rate * depth;
            let two_pi = std::f32::consts::TAU;
            let phase_inc = rate_hz / self.sample_rate;
            for s in buf.iter_mut() {
                self.flanger_phase += phase_inc;
                if self.flanger_phase >= 1.0 {
                    self.flanger_phase -= 1.0;
                }
                let lfo = 0.5 + 0.5 * (self.flanger_phase * two_pi).sin();
                let delay_samp = (min_delay + max_extra * lfo).clamp(1.0, (flen - 2) as f32);
                let delay_i = delay_samp.floor() as usize;
                let frac = delay_samp - delay_i as f32;
                let i0 = (self.flanger_pos + flen - delay_i) % flen;
                let i1 = (i0 + flen - 1) % flen;
                let wet = self.flanger[i0] * (1.0 - frac) + self.flanger[i1] * frac;
                self.flanger[self.flanger_pos] = (*s + wet * fb).clamp(-1.5, 1.5);
                self.flanger_pos = (self.flanger_pos + 1) % flen;
                *s = *s * dry + wet * mix;
            }
        }

        if p.delay_mix > 0.001 || p.delay_fb > 0.001 {
            let dlen = self.delay.len();
            let delay_sec = 0.05 + p.delay_time * 0.70;
            let ds = ((delay_sec * self.sample_rate) as usize).clamp(1, dlen - 1);
            let fb = p.delay_fb.min(0.92);
            let mix = p.delay_mix;
            let dry = 1.0 - mix;
            for s in buf.iter_mut() {
                let read = (self.delay_pos + dlen - ds) % dlen;
                let wet = self.delay[read];
                self.delay[self.delay_pos] = *s + wet * fb;
                self.delay_pos = (self.delay_pos + 1) % dlen;
                if mix > 0.001 {
                    *s = *s * dry + wet * mix;
                }
            }
        }

        if p.reverb_mix > 0.001 {
            let rlen = self.reverb.len();
            let size = p.reverb_size;
            let base = ((0.018 + 0.040 * size) * self.sample_rate) as usize;
            let taps = [
                base.clamp(1, rlen - 1),
                ((base as f32 * 1.7) as usize).clamp(1, rlen - 1),
                ((base as f32 * 2.5) as usize).clamp(1, rlen - 1),
                ((base as f32 * 3.4) as usize).clamp(1, rlen - 1),
            ];
            let gains = [0.55f32, 0.40, 0.30, 0.22];
            let fb = 0.25 + 0.45 * size;
            let mix = p.reverb_mix;
            let dry = 1.0 - mix;
            let blend = if size > 0.05 { size.clamp(0.0, 1.0) } else { 0.0 };
            for s in buf.iter_mut() {
                let mut wet = 0.0f32;
                for (tap, gain) in taps.iter().zip(gains.iter()) {
                    let read = (self.reverb_pos + rlen - tap) % rlen;
                    wet += self.reverb[read] * gain;
                }
                if blend > 0.0 {
                    let soft = 0.5 * (wet + self.reverb_prev);
                    wet = wet * (1.0 - blend) + soft * blend;
                }
                self.reverb_prev = wet;
                self.reverb[self.reverb_pos] = *s * 0.7 + wet * fb;
                self.reverb_pos = (self.reverb_pos + 1) % rlen;
                *s = *s * dry + wet * mix;
            }
        }

        if input_hot && p.is_wet() {
            self.tail_hold = tail_samples(&p, self.sample_rate);
        } else {
            self.tail_hold = self.tail_hold.saturating_sub(buf.len() as u32);
        }
    }
}

/// How long to keep running after the last dry input, given current wet amounts.
fn tail_samples(p: &FxParams, sample_rate: f32) -> u32 {
    let mut sec = 0.0f32;
    if p.delay_mix > 0.001 {
        let delay_sec = 0.05 + p.delay_time * 0.70;
        let fb = p.delay_fb.clamp(0.0, 0.92);
        let repeats = if fb < 0.05 {
            2.0
        } else {
            (0.002f32.ln() / fb.max(0.05).ln()).clamp(2.0, 24.0)
        };
        sec = sec.max(delay_sec * repeats);
    }
    if p.reverb_mix > 0.001 {
        sec = sec.max(0.30 + p.reverb_size * 1.4);
    }
    if p.flanger_mix > 0.001 {
        sec = sec.max(0.08);
    }
    (sec * sample_rate).ceil() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_leave_the_audible_mix_dry() {
        // Defaults match Python: delay_fb is 0.35 so the delay line still runs
        // (is_dry is false), but mix is 0 so the audible buffer is unchanged.
        assert!(!FxParams::default().is_bypassed());
        assert!(FxParams::default().delay_mix <= 0.001);
        assert!(FxParams::default().reverb_mix <= 0.001);
        assert!(FxParams::default().drive <= 0.001);
    }

    #[test]
    fn bypassed_unit_leaves_signal_untouched() {
        let mut fx = FxUnit::new(48_000.0);
        let mut buf = [0.5f32; 64];
        fx.process(&mut buf);
        assert!(buf.iter().all(|v| (*v - 0.5).abs() < 1e-6));
    }

    #[test]
    fn drive_compresses_peaks_without_exploding() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            drive: 1.0,
            ..FxParams::default()
        });
        let mut buf = [0.8f32; 32];
        fx.process(&mut buf);
        assert!(buf.iter().all(|v| v.abs() <= 1.0));
        // Saturation lifts a steady 0.8 toward the rail.
        assert!(buf[0] > 0.8);
    }

    #[test]
    fn delay_repeats_an_impulse_later() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            delay_time: 0.0, // 50 ms
            delay_fb: 0.0,
            delay_mix: 1.0,
            ..FxParams::default()
        });
        let expect = (0.05 * 48_000.0) as usize;
        let mut buf = vec![0.0f32; expect * 2];
        buf[0] = 1.0;
        fx.process(&mut buf);
        assert!(buf[expect] > 0.5, "impulse should reappear one delay later");
    }

    #[test]
    fn reset_clears_the_tail() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            delay_time: 0.0,
            delay_fb: 0.5,
            delay_mix: 1.0,
            ..FxParams::default()
        });
        let mut buf = vec![0.0f32; 256];
        buf[0] = 1.0;
        fx.process(&mut buf);
        fx.reset();
        let mut quiet = vec![0.0f32; (0.05 * 48_000.0) as usize + 8];
        fx.process(&mut quiet);
        assert!(quiet.iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn params_are_clamped() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            drive: 9.0,
            delay_mix: -3.0,
            ..FxParams::default()
        });
        assert_eq!(fx.params().drive, 1.0);
        assert_eq!(fx.params().delay_mix, 0.0);
    }

    #[test]
    fn flanger_wet_moves_an_impulse() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            flanger_mix: 1.0,
            flanger_rate: 0.0,
            flanger_depth: 0.0,
            flanger_fb: 0.0,
            ..FxParams::default()
        });
        // Rate 0 still advances a tiny bit; depth 0 parks the delay at ~1.2 ms.
        let expect = (0.0012 * 48_000.0) as usize;
        let mut buf = vec![0.0f32; expect + 8];
        buf[0] = 1.0;
        fx.process(&mut buf);
        let peak = buf.iter().copied().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.3, "flanger should reprint the impulse, peak={peak}");
        assert!(buf[0].abs() < 0.15, "full mix should replace the dry hit");
    }

    #[test]
    fn flanger_mix_zero_is_silent_in_bypass() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            flanger_mix: 0.0,
            flanger_rate: 1.0,
            flanger_depth: 1.0,
            ..FxParams::default()
        });
        let mut buf = [0.4f32; 64];
        fx.process(&mut buf);
        assert!(buf.iter().all(|v| (*v - 0.4).abs() < 1e-5));
    }

    #[test]
    fn wet_impulse_keeps_a_tail_flag() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            delay_time: 0.0,
            delay_fb: 0.0,
            delay_mix: 1.0,
            ..FxParams::default()
        });
        let mut buf = [0.0f32; 64];
        buf[0] = 1.0;
        fx.process(&mut buf);
        assert!(fx.has_tail(), "a wet hit should keep the tank spinning");
        fx.reset();
        assert!(!fx.has_tail(), "panic / reset must kill the leftover echo");
    }

    #[test]
    fn tail_flag_expires_after_enough_silence() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            delay_time: 0.0,
            delay_fb: 0.0,
            delay_mix: 1.0,
            ..FxParams::default()
        });
        let mut hit = [0.0f32; 64];
        hit[0] = 1.0;
        fx.process(&mut hit);
        assert!(fx.has_tail());
        let mut quiet = vec![0.0f32; 2048];
        for _ in 0..16 {
            quiet.fill(0.0);
            fx.process(&mut quiet);
        }
        assert!(!fx.has_tail(), "a one-repeat delay should finish and go idle");
    }
}
