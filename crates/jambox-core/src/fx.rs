//! Insert FX: drive → delay → short multi-tap tank.
//!
//! One [`FxUnit`] per wavetable voice, per drum model, per drum group, and one on
//! the master bus (see `PLAN.md` "Effects (insert + kit group + bus)"). Buffers are
//! sized in [`FxUnit::new`]; `process` never allocates.

const DELAY_MAX_SEC: f32 = 0.80;
const REVERB_MAX_SEC: f32 = 0.55;

/// Plain-old-data FX amounts, all 0..1. Cheap to copy across the command ring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxParams {
    pub drive: f32,
    pub delay_time: f32,
    pub delay_fb: f32,
    pub delay_mix: f32,
    pub reverb_size: f32,
    pub reverb_mix: f32,
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
        }
    }
}

impl FxParams {
    /// True when the unit would be a no-op, so `process` can be skipped entirely.
    pub fn is_bypassed(&self) -> bool {
        self.drive <= 0.001 && self.delay_mix <= 0.001 && self.reverb_mix <= 0.001
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
}

impl FxUnit {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(8000.0);
        let dlen = (sample_rate * DELAY_MAX_SEC) as usize + 64;
        let rlen = (sample_rate * REVERB_MAX_SEC) as usize + 64;
        Self {
            params: FxParams::default(),
            sample_rate,
            delay: vec![0.0; dlen],
            delay_pos: 0,
            reverb: vec![0.0; rlen],
            reverb_pos: 0,
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
        };
    }

    /// Clear tails — used on panic / all-notes-off so a stale echo can't leak.
    pub fn reset(&mut self) {
        self.delay.iter_mut().for_each(|v| *v = 0.0);
        self.reverb.iter_mut().for_each(|v| *v = 0.0);
        self.delay_pos = 0;
        self.reverb_pos = 0;
    }

    /// Process in place. Allocation-free; safe to call from the audio thread.
    pub fn process(&mut self, buf: &mut [f32]) {
        if buf.is_empty() {
            return;
        }
        let p = self.params;

        if p.drive > 0.001 {
            let amount = 1.0 + p.drive * 12.0;
            let norm = 1.0 / amount.tanh().max(0.25);
            for s in buf.iter_mut() {
                *s = (*s * amount).tanh() * norm;
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
            for s in buf.iter_mut() {
                let mut wet = 0.0f32;
                for (tap, gain) in taps.iter().zip(gains.iter()) {
                    let read = (self.reverb_pos + rlen - tap) % rlen;
                    wet += self.reverb[read] * gain;
                }
                self.reverb[self.reverb_pos] = *s * 0.7 + wet * fb;
                self.reverb_pos = (self.reverb_pos + 1) % rlen;
                *s = *s * dry + wet * mix;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_are_bypassed() {
        assert!(FxParams::default().is_bypassed());
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
}
