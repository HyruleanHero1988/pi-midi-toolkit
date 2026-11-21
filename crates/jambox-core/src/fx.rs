//! Insert FX: drive → chorus → flanger → phaser → delay → short multi-tap tank.
//!
//! One [`FxUnit`] per wavetable voice, per drum model, per drum group, and one on
//! the master bus (see `PLAN.md` "Effects (insert + kit group + bus)"). Buffers are
//! sized in [`FxUnit::new`]; `process` never allocates.

const DELAY_MAX_SEC: f32 = 1.0;
const REVERB_MAX_SEC: f32 = 0.55;
/// Classic analog flanger window (~1–12 ms). Sized once in `new`.
const FLANGER_MAX_SEC: f32 = 0.020;
/// Chorus delay window (base + voice offsets + shallow LFO).
const CHORUS_MAX_SEC: f32 = 0.050;
/// BBD-style first tap (~22 ms); additional voices stagger after this.
const CHORUS_BASE_SEC: f32 = 0.022;
/// Per-voice static offset (seconds) for ensemble width.
const CHORUS_VOICE_OFFSET_SEC: f32 = 0.004;
/// Peak LFO travel at `chorus_depth == 1` — keep shallow (CE/Juno class).
const CHORUS_DEPTH_SEC: f32 = 0.0010;
/// Parallel detuned copies mixed into dry; keeps level sensible.
const CHORUS_ENSEMBLE_GAIN: f32 = 0.62;
const CHORUS_VOICES: usize = 3;
/// Serial first-order all-pass stages (Phase 90–class; stronger combing).
const PHASER_STAGES: usize = 10;
/// Lowest sweep corner when the LFO is down (Hz).
const PHASER_FREQ_HZ_MIN: f32 = 100.0;
/// Added Hz at full depth when the LFO is up (top ~1.6–3 kHz, classic pedal range).
const PHASER_FREQ_HZ_SPAN: f32 = 3_200.0;
/// Cap on phaser feedback — higher `phaser_fb` tightens the jet / resonant peaks.
const PHASER_FB_MAX: f32 = 0.92;
/// Drive oversampling factor around the tube nonlinearity.
const DRIVE_OS: usize = 2;

/// Delay gap at `delay_time == 0` (seconds). Kept public for tempo-grid snaps.
pub const DELAY_TIME_SEC_MIN: f32 = 0.05;
/// Extra seconds added when `delay_time == 1` (max gap = min + span).
pub const DELAY_TIME_SEC_SPAN: f32 = 0.70;
/// Flanger LFO Hz at `flanger_rate == 0`.
pub const FLANGER_RATE_HZ_MIN: f32 = 0.08;
/// Extra Hz when `flanger_rate == 1`.
pub const FLANGER_RATE_HZ_SPAN: f32 = 6.0;
/// Chorus LFO Hz at `chorus_rate == 0` (slow ensemble).
pub const CHORUS_RATE_HZ_MIN: f32 = 0.12;
/// Extra Hz when `chorus_rate == 1`.
pub const CHORUS_RATE_HZ_SPAN: f32 = 0.38;
/// Phaser LFO Hz at `phaser_rate == 0`.
pub const PHASER_RATE_HZ_MIN: f32 = 0.10;
/// Extra Hz when `phaser_rate == 1`.
pub const PHASER_RATE_HZ_SPAN: f32 = 4.0;

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
    pub chorus_mix: f32,
    pub chorus_rate: f32,
    pub chorus_depth: f32,
    pub phaser_mix: f32,
    pub phaser_rate: f32,
    pub phaser_depth: f32,
    pub phaser_fb: f32,
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
            chorus_mix: 0.0,
            chorus_rate: 0.35,
            chorus_depth: 0.40,
            phaser_mix: 0.0,
            phaser_rate: 0.35,
            phaser_depth: 0.85,
            phaser_fb: 0.55,
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
            && self.chorus_mix <= 0.001
            && self.phaser_mix <= 0.001
    }

    /// True when a wet mix would still be audible after the dry voice dies.
    pub fn is_wet(&self) -> bool {
        self.delay_mix > 0.001
            || self.reverb_mix > 0.001
            || self.flanger_mix > 0.001
            || self.chorus_mix > 0.001
            || self.phaser_mix > 0.001
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
    chorus: Vec<f32>,
    chorus_pos: usize,
    chorus_phase: f32,
    phaser_z: [f32; PHASER_STAGES],
    phaser_phase: f32,
    phaser_fb_z: f32,
    /// DC blocker state after the tube drive.
    drive_dc_x: f32,
    drive_dc_y: f32,
    /// Samples of silence still worth running so delay / reverb can finish.
    tail_hold: u32,
}

impl FxUnit {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(8000.0);
        let dlen = (sample_rate * DELAY_MAX_SEC) as usize + 64;
        let rlen = (sample_rate * REVERB_MAX_SEC) as usize + 64;
        let flen = (sample_rate * FLANGER_MAX_SEC) as usize + 8;
        let clen = (sample_rate * CHORUS_MAX_SEC) as usize + 8;
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
            chorus: vec![0.0; clen.max(64)],
            chorus_pos: 0,
            chorus_phase: 0.0,
            phaser_z: [0.0; PHASER_STAGES],
            phaser_phase: 0.0,
            phaser_fb_z: 0.0,
            drive_dc_x: 0.0,
            drive_dc_y: 0.0,
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
            chorus_mix: params.chorus_mix.clamp(0.0, 1.0),
            chorus_rate: params.chorus_rate.clamp(0.0, 1.0),
            chorus_depth: params.chorus_depth.clamp(0.0, 1.0),
            phaser_mix: params.phaser_mix.clamp(0.0, 1.0),
            phaser_rate: params.phaser_rate.clamp(0.0, 1.0),
            phaser_depth: params.phaser_depth.clamp(0.0, 1.0),
            phaser_fb: params.phaser_fb.clamp(0.0, 1.0),
        };
    }

    /// Clear tails — used on panic / all-notes-off so a stale echo can't leak.
    pub fn reset(&mut self) {
        self.delay.iter_mut().for_each(|v| *v = 0.0);
        self.reverb.iter_mut().for_each(|v| *v = 0.0);
        self.flanger.iter_mut().for_each(|v| *v = 0.0);
        self.chorus.iter_mut().for_each(|v| *v = 0.0);
        self.delay_pos = 0;
        self.reverb_pos = 0;
        self.reverb_prev = 0.0;
        self.flanger_pos = 0;
        self.flanger_phase = 0.0;
        self.chorus_pos = 0;
        self.chorus_phase = 0.0;
        self.phaser_z = [0.0; PHASER_STAGES];
        self.phaser_phase = 0.0;
        self.phaser_fb_z = 0.0;
        self.drive_dc_x = 0.0;
        self.drive_dc_y = 0.0;
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
            process_tube_drive(
                buf,
                p.drive,
                &mut self.drive_dc_x,
                &mut self.drive_dc_y,
            );
        }

        if p.chorus_mix > 0.001 {
            let clen = self.chorus.len();
            let rate_hz = CHORUS_RATE_HZ_MIN + p.chorus_rate * CHORUS_RATE_HZ_SPAN;
            let depth = p.chorus_depth;
            let mix = perceptual_insert_mix(p.chorus_mix);
            let dry = 1.0 - mix;
            let center0 = CHORUS_BASE_SEC * self.sample_rate;
            let voice_step = CHORUS_VOICE_OFFSET_SEC * self.sample_rate;
            let travel = CHORUS_DEPTH_SEC * self.sample_rate * depth;
            let two_pi = std::f32::consts::TAU;
            let phase_inc = rate_hz / self.sample_rate;
            for s in buf.iter_mut() {
                self.chorus_phase += phase_inc;
                if self.chorus_phase >= 1.0 {
                    self.chorus_phase -= 1.0;
                }
                let input = *s;
                let mut ensemble = 0.0f32;
                for v in 0..CHORUS_VOICES {
                    let phase_off = v as f32 * two_pi / CHORUS_VOICES as f32;
                    let lfo = (self.chorus_phase * two_pi + phase_off).sin();
                    let base = center0 + v as f32 * voice_step;
                    let delay_samp = (base + travel * lfo).clamp(1.0, (clen - 2) as f32);
                    ensemble += read_delay_frac(&self.chorus, self.chorus_pos, delay_samp);
                }
                ensemble /= CHORUS_VOICES as f32;
                self.chorus[self.chorus_pos] = input.clamp(-1.5, 1.5);
                self.chorus_pos = (self.chorus_pos + 1) % clen;
                // Juno/CE thickening: dry plus low-level staggered copies, not a
                // single sweeping comb against dry (which reads as flange/phaser).
                let thickened = input + ensemble * CHORUS_ENSEMBLE_GAIN;
                *s = input * dry + thickened * mix;
            }
        }

        if p.flanger_mix > 0.001 {
            let flen = self.flanger.len();
            let rate_hz = FLANGER_RATE_HZ_MIN + p.flanger_rate * FLANGER_RATE_HZ_SPAN;
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
                let wet = read_delay_frac(&self.flanger, self.flanger_pos, delay_samp);
                self.flanger[self.flanger_pos] = (*s + wet * fb).clamp(-1.5, 1.5);
                self.flanger_pos = (self.flanger_pos + 1) % flen;
                *s = *s * dry + wet * mix;
            }
        }

        if p.phaser_mix > 0.001 {
            let rate_hz = PHASER_RATE_HZ_MIN + p.phaser_rate * PHASER_RATE_HZ_SPAN;
            let mix = perceptual_insert_mix(p.phaser_mix);
            let depth = p.phaser_depth * (0.45 + 0.55 * mix);
            let fb = p.phaser_fb.min(PHASER_FB_MAX) * (0.5 + 0.5 * mix);
            let phase_inc = rate_hz / self.sample_rate;
            let two_pi = std::f32::consts::TAU;
            let sweep_hz = depth * PHASER_FREQ_HZ_SPAN;
            let f_lo = PHASER_FREQ_HZ_MIN;
            let f_hi = PHASER_FREQ_HZ_MIN + sweep_hz.max(120.0);
            for s in buf.iter_mut() {
                let dry = *s;
                self.phaser_phase += phase_inc;
                if self.phaser_phase >= 1.0 {
                    self.phaser_phase -= 1.0;
                }
                let lfo = 0.5 + 0.5 * (self.phaser_phase * two_pi).sin();
                let center_hz = f_lo + (f_hi - f_lo) * lfo;
                let coeff = phaser_ap_coeff(center_hz, self.sample_rate);
                let mut x = (dry + self.phaser_fb_z * fb).clamp(-2.0, 2.0);
                for z in self.phaser_z.iter_mut() {
                    // First-order all-pass: y = a*x + z; z = x - a*y
                    let y = coeff * x + *z;
                    *z = x - coeff * y;
                    x = y;
                }
                self.phaser_fb_z = x.clamp(-1.5, 1.5);
                // Classic insert: notches come from ~50/50 dry + phased, not all-pass alone.
                let classic = 0.5 * (dry + x);
                *s = dry * (1.0 - mix) + classic * mix;
            }
        }

        if p.delay_mix > 0.001 || p.delay_fb > 0.001 {
            let dlen = self.delay.len();
            let delay_sec = DELAY_TIME_SEC_MIN + p.delay_time * DELAY_TIME_SEC_SPAN;
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

/// Slider 0..1 → wet amount; mid positions already feel clearly effected.
#[inline]
fn perceptual_insert_mix(mix: f32) -> f32 {
    let mix = mix.clamp(0.0, 1.0);
    1.0 - (1.0 - mix).powf(1.65)
}

/// Bilinear first-order all-pass coefficient for notch center `center_hz`.
#[inline]
fn phaser_ap_coeff(center_hz: f32, sample_rate: f32) -> f32 {
    let t = (std::f32::consts::PI * center_hz / sample_rate).tan();
    ((1.0 - t) / (1.0 + t)).clamp(0.05, 0.95)
}

#[inline]
fn read_delay_frac(buf: &[f32], write_pos: usize, delay_samp: f32) -> f32 {
    let len = buf.len();
    let delay_i = delay_samp.floor() as usize;
    let frac = delay_samp - delay_i as f32;
    let i0 = (write_pos + len - delay_i) % len;
    let i1 = (i0 + len - 1) % len;
    buf[i0] * (1.0 - frac) + buf[i1] * frac
}

/// Asymmetric soft-knee tube clipper (H2-rich) with mild pre-emphasis, DC block,
/// partial level makeup, and 2× oversampling around the nonlinearity.
///
/// `drive` is saturation amount only (full insert). Dry/wet crossfading on this
/// knob had made moderate bus levels sound almost clean — see PR fixing bus DRIVE.
fn process_tube_drive(buf: &mut [f32], drive: f32, dc_x: &mut f32, dc_y: &mut f32) {
    let t = drive.clamp(0.0, 1.0);
    // Quadratic gain: warm bite by ~0.4, obvious crunch by 1.0 on typical bus levels.
    let gain = 1.0 + t * t * 22.0;
    // Partial makeup so level stays sensible without undoing harmonic color.
    let makeup = 1.0 / (1.0 + 0.30 * t * t);
    let pre = 0.14 * t;
    let r = 0.995f32;

    for s in buf.iter_mut() {
        let x0 = *s;
        let mut acc = 0.0f32;
        let mut prev = x0;
        for step in 0..DRIVE_OS {
            let x = if step == 0 {
                x0
            } else {
                0.5 * (prev + x0)
            };
            prev = x0;
            let emphasized = x * (1.0 + pre);
            let hot = emphasized * gain;
            let clipped = dual_knee_clip(hot, t);
            let shaped = clipped * makeup;
            let y = shaped - *dc_x + r * *dc_y;
            *dc_x = shaped;
            *dc_y = y;
            acc += y;
        }
        *s = acc / DRIVE_OS as f32;
    }
}

/// Positive half saturates earlier/firmer; negative later/softer → mostly H2.
/// Knees tighten as `drive` rises so bus-level signals hit the tube sooner.
#[inline]
fn dual_knee_clip(x: f32, drive: f32) -> f32 {
    let knee_p = 0.90 - 0.58 * drive;
    let knee_n = 1.18 - 0.48 * drive;
    let span_p = 0.48 + 0.14 * drive;
    let span_n = 1.08 + 0.18 * drive;
    if x >= 0.0 {
        soft_knee(x, knee_p, span_p)
    } else {
        -soft_knee(-x, knee_n, span_n)
    }
}

#[inline]
fn soft_knee(x: f32, knee: f32, span: f32) -> f32 {
    if x <= knee {
        x
    } else {
        let d = x - knee;
        knee + d * span / (span + d)
    }
}

/// How long to keep running after the last dry input, given current wet amounts.
fn tail_samples(p: &FxParams, sample_rate: f32) -> u32 {
    let mut sec = 0.0f32;
    if p.delay_mix > 0.001 {
        let delay_sec = DELAY_TIME_SEC_MIN + p.delay_time * DELAY_TIME_SEC_SPAN;
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
    if p.chorus_mix > 0.001 {
        sec = sec.max(0.10);
    }
    if p.phaser_mix > 0.001 {
        sec = sec.max(0.06);
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
        assert!(FxParams::default().chorus_mix <= 0.001);
        assert!(FxParams::default().phaser_mix <= 0.001);
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
        assert!(buf.iter().all(|v| v.abs() <= 1.5));
        // Tube drive should still be audible / non-zero.
        assert!(buf.iter().any(|v| v.abs() > 0.1));
    }

    fn sine_buffer(freq_hz: f32, amp: f32, sample_rate: f32, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames];
        for (i, s) in buf.iter_mut().enumerate() {
            *s = (i as f32 * freq_hz / sample_rate * std::f32::consts::TAU).sin() * amp;
        }
        buf
    }

  /// Single-bin Goertzel magnitude at `freq_hz` (optionally a harmonic of the base tone).
    fn goertzel_mag(buf: &[f32], freq_hz: f32, sample_rate: f32) -> f32 {
        let n = buf.len();
        let omega = std::f32::consts::TAU * freq_hz / sample_rate;
        let coeff = 2.0 * omega.cos();
        let mut s1 = 0.0f32;
        let mut s2 = 0.0f32;
        for &x in buf {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let real = s1 - s2 * omega.cos();
        let imag = s2 * omega.sin();
        (real * real + imag * imag).sqrt() / n as f32 * 2.0
    }

    fn measure_h2_over_h1(
        fx: &mut FxUnit,
        drive: f32,
        freq_hz: f32,
        amp: f32,
        sample_rate: f32,
    ) -> f32 {
        let frames = (sample_rate * 0.5) as usize;
        fx.set_params(FxParams {
            drive,
            ..FxParams::default()
        });
        let mut buf = sine_buffer(freq_hz, amp, sample_rate, frames);
        fx.process(&mut buf);
        let h1 = goertzel_mag(&buf, freq_hz, sample_rate).max(1e-8);
        let h2 = goertzel_mag(&buf, freq_hz * 2.0, sample_rate);
        h2 / h1
    }

    #[test]
    fn drive_second_harmonic_rises_with_amount() {
        let sr = 48_000.0;
        let mut fx_lo = FxUnit::new(sr);
        let mut fx_mid = FxUnit::new(sr);
        let mut fx_hi = FxUnit::new(sr);
        let amp = 0.22f32;
        let low = measure_h2_over_h1(&mut fx_lo, 0.35, 440.0, amp, sr);
        let mid = measure_h2_over_h1(&mut fx_mid, 0.55, 440.0, amp, sr);
        let high = measure_h2_over_h1(&mut fx_hi, 1.0, 440.0, amp, sr);
        assert!(
            mid > low * 2.5 && high > low * 6.0,
            "H2/H1 should climb with drive: low={low:.5} mid={mid:.5} high={high:.5}"
        );
        assert!(
            mid > 0.06,
            "moderate drive must be clearly non-clean at bus levels, mid={mid:.5}"
        );
        assert!(
            high > 0.07,
            "full drive should be crunchy (compression may trim H2 vs mid), high={high:.5}"
        );
    }

    #[test]
    fn drive_moderate_changes_waveform_from_clean() {
        let sr = 48_000.0;
        let frames = 4096usize;
        let freq = 330.0f32;
        let amp = 0.2f32;
        let clean = sine_buffer(freq, amp, sr, frames);
        let mut driven = clean.clone();
        let mut fx = FxUnit::new(sr);
        fx.set_params(FxParams {
            drive: 0.5,
            ..FxParams::default()
        });
        fx.process(&mut driven);
        let mut diff_sq = 0.0f32;
        for (a, b) in clean.iter().zip(driven.iter()) {
            let d = a - b;
            diff_sq += d * d;
        }
        let rms_diff = (diff_sq / frames as f32).sqrt();
        assert!(
            rms_diff > 0.008,
            "drive 0.5 should audibly reshape a bus-level tone, rms_diff={rms_diff:.5}"
        );
    }

    #[test]
    fn drive_zero_is_clean() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            drive: 0.0,
            ..FxParams::default()
        });
        let mut buf = [0.4f32; 64];
        fx.process(&mut buf);
        assert!(buf.iter().all(|v| (*v - 0.4).abs() < 1e-5));
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
            chorus_mix: 2.0,
            phaser_mix: -1.0,
            ..FxParams::default()
        });
        assert_eq!(fx.params().drive, 1.0);
        assert_eq!(fx.params().delay_mix, 0.0);
        assert_eq!(fx.params().chorus_mix, 1.0);
        assert_eq!(fx.params().phaser_mix, 0.0);
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
    fn chorus_wet_moves_an_impulse() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            chorus_mix: 1.0,
            chorus_rate: 0.0,
            chorus_depth: 0.0,
            ..FxParams::default()
        });
        let expect = (CHORUS_BASE_SEC * 48_000.0) as usize;
        let mut buf = vec![0.0f32; expect + 64];
        buf[0] = 1.0;
        fx.process(&mut buf);
        assert!(buf[0].abs() > 0.9, "dry hit stays at the attack");
        let tail_peak = buf[expect..]
            .iter()
            .copied()
            .fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(
            tail_peak > 0.12,
            "ensemble should reprint after ~{expect} samples, tail_peak={tail_peak}"
        );
    }

    #[test]
    fn chorus_modulation_stays_in_chorus_delay_window() {
        let sr = 48_000.0;
        let min_ms = CHORUS_BASE_SEC * 1000.0;
        let max_ms = (CHORUS_BASE_SEC
            + CHORUS_VOICE_OFFSET_SEC * (CHORUS_VOICES - 1) as f32
            + CHORUS_DEPTH_SEC)
            * 1000.0;
        assert!(min_ms >= 15.0, "chorus min delay {min_ms} ms too short (flanger territory)");
        assert!(max_ms <= 50.0, "chorus max delay {max_ms} ms exceeds buffer window");
        assert!(
            CHORUS_DEPTH_SEC <= 0.002,
            "chorus depth {CHORUS_DEPTH_SEC}s too wide — audible whoosh"
        );

        let mut fx = FxUnit::new(sr);
        fx.set_params(FxParams {
            chorus_mix: 1.0,
            chorus_rate: 1.0,
            chorus_depth: 1.0,
            ..FxParams::default()
        });
        let flanger_end = (0.008 * sr) as usize;
        let chorus_lo = (CHORUS_BASE_SEC * sr) as usize;
        let n = ((CHORUS_BASE_SEC + CHORUS_DEPTH_SEC) * sr) as usize + 96;
        for _ in 0..8 {
            fx.reset();
            let mut buf = vec![0.0f32; n];
            buf[0] = 1.0;
            fx.process(&mut buf);
            let peak_i = buf[chorus_lo..]
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.abs().partial_cmp(&b.abs()).unwrap())
                .map(|(i, _)| i + chorus_lo)
                .unwrap_or(chorus_lo);
            assert!(
                peak_i >= chorus_lo.saturating_sub(4),
                "ensemble chorus peak at sample {peak_i} left the chorus window"
            );
            // Skip the dry attack at sample 0; we're checking for short-delay leakage.
            let flanger_zone_peak = buf[1..flanger_end]
                .iter()
                .copied()
                .fold(0.0f32, |a, b| a.max(b.abs()));
            assert!(
                flanger_zone_peak < 0.12,
                "chorus leaked into flanger window, peak={flanger_zone_peak}"
            );
        }
    }

    fn rich_saw(sr: f32, frames: usize, fund: f32) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames];
        for (i, s) in buf.iter_mut().enumerate() {
            let t = i as f32 / sr;
            let mut sum = 0.0f32;
            for h in 1..=12 {
                sum += (t * fund * h as f32 * std::f32::consts::TAU).sin() / (h as f32 * 0.85);
            }
            *s = sum * 0.14;
        }
        buf
    }

    fn block_rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|v| v * v).sum::<f32>() / buf.len().max(1) as f32).sqrt()
    }

    #[test]
    fn phaser_synth_swoosh_is_audible_at_bus_slider_levels() {
        let sr = 48_000.0;
        let params = FxParams {
            phaser_mix: 0.72,
            phaser_rate: 0.35,
            phaser_depth: 0.85,
            phaser_fb: 0.55,
            ..FxParams::default()
        };
        let rate_hz = PHASER_RATE_HZ_MIN + params.phaser_rate * PHASER_RATE_HZ_SPAN;
        let frames = (sr * 2.5) as usize;
        let dry = rich_saw(sr, frames, 110.0);
        let in_rms = block_rms(&dry);
        let block = ((sr / rate_hz.max(0.08)) / 10.0) as usize;
        let block = block.clamp(256, 4096);

        let mut fx = FxUnit::new(sr);
        fx.set_params(params);
        let mut wet = dry.clone();
        fx.process(&mut wet);
        let mut blocks = Vec::new();
        for chunk in wet.chunks(block) {
            blocks.push(block_rms(chunk));
        }
        let swing = blocks
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max)
            - blocks.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            swing / in_rms > 0.045,
            "phaser should breathe on a synth saw, swing/in_rms={}",
            swing / in_rms
        );
        let peak_delta = dry
            .iter()
            .zip(wet.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(peak_delta > 0.08, "peak sample delta too small: {peak_delta}");
    }

    #[test]
    fn chorus_thickens_saw_without_flanger_comb_sweep() {
        let sr = 48_000.0;
        let frames = (sr * 1.25) as usize;
        let src = rich_saw(sr, frames, 165.0);
        let dry_rms = block_rms(&src);

        let mut chorus_fx = FxUnit::new(sr);
        chorus_fx.set_params(FxParams {
            chorus_mix: 0.8,
            chorus_rate: 0.35,
            chorus_depth: 0.55,
            ..FxParams::default()
        });
        let mut chorus_buf = src.clone();
        chorus_fx.process(&mut chorus_buf);
        let chorus_rms = block_rms(&chorus_buf);
        let dry_peak = src.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let chorus_peak = chorus_buf.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        assert!(
            chorus_rms > dry_rms * 0.98 && chorus_peak > dry_peak * 1.02,
            "chorus should thicken, dry_rms={dry_rms} wet_rms={chorus_rms} dry_pk={dry_peak} wet_pk={chorus_peak}"
        );

        let mut flanger_fx = FxUnit::new(sr);
        flanger_fx.set_params(FxParams {
            flanger_mix: 0.8,
            flanger_rate: 0.5,
            flanger_depth: 0.85,
            flanger_fb: 0.4,
            ..FxParams::default()
        });
        let mut flanger_buf = src.clone();
        flanger_fx.process(&mut flanger_buf);

        let block = 512usize;
        fn mean_abs_delta(buf: &[f32], block: usize) -> f32 {
            let mut deltas = Vec::new();
            for w in buf.windows(block).skip(1) {
                let prev = block_rms(&w[..block]);
                deltas.push(prev);
            }
            if deltas.len() < 3 {
                return 0.0;
            }
            let mean = deltas.iter().sum::<f32>() / deltas.len() as f32;
            deltas.iter().map(|d| (d - mean).abs()).sum::<f32>() / deltas.len() as f32
        }
        let chorus_ripple = mean_abs_delta(&chorus_buf, block);
        let flanger_ripple = mean_abs_delta(&flanger_buf, block);
        assert!(
            chorus_ripple < flanger_ripple * 0.55,
            "chorus should wobble less than flanger: chorus={chorus_ripple} flanger={flanger_ripple}"
        );
    }

    #[test]
    fn chorus_mix_zero_is_silent_in_bypass() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(FxParams {
            chorus_mix: 0.0,
            chorus_rate: 1.0,
            chorus_depth: 1.0,
            ..FxParams::default()
        });
        let mut buf = [0.4f32; 64];
        fx.process(&mut buf);
        assert!(buf.iter().all(|v| (*v - 0.4).abs() < 1e-5));
    }

    fn fill_phaser_noise(buf: &mut [f32], seed: u32) {
        let mut s = seed;
        for x in buf.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *x = (s as f32 / u32::MAX as f32) * 2.0 - 1.0;
        }
    }

    fn measure_phaser_noise_rms_gain(fx: &mut FxUnit, sample_rate: f32, seed: u32) -> f32 {
        let frames = (sample_rate * 0.35) as usize;
        let mut buf = vec![0.0f32; frames];
        fill_phaser_noise(&mut buf, seed);
        let in_rms = (buf.iter().map(|v| v * v).sum::<f32>() / frames as f32).sqrt();
        fx.process(&mut buf);
        let out_rms = (buf.iter().map(|v| v * v).sum::<f32>() / frames as f32).sqrt();
        out_rms / in_rms
    }

    fn measure_phaser_tone_gain_at_lfo(fx: &mut FxUnit, freq_hz: f32, sample_rate: f32) -> f32 {
        let frames = 4096usize;
        let mut buf = vec![0.0f32; frames];
        for (i, s) in buf.iter_mut().enumerate() {
            *s = (i as f32 * freq_hz / sample_rate * std::f32::consts::TAU).sin();
        }
        fx.process(&mut buf);
        let out_rms = (buf.iter().map(|v| v * v).sum::<f32>() / frames as f32).sqrt();
        out_rms / std::f32::consts::FRAC_1_SQRT_2
    }

    fn phaser_params_loud() -> FxParams {
        FxParams {
            phaser_mix: 1.0,
            phaser_rate: 0.0,
            phaser_depth: 1.0,
            phaser_fb: 0.55,
            ..FxParams::default()
        }
    }

    #[test]
    fn phaser_ap_coeff_tracks_center_frequency() {
        let low = phaser_ap_coeff(120.0, 48_000.0);
        let high = phaser_ap_coeff(2_400.0, 48_000.0);
        assert!((low - high).abs() > 0.1, "coeff should move with notch center");
    }

    #[test]
    fn phaser_full_mix_produces_deep_notches() {
        let sr = 48_000.0;
        let params = phaser_params_loud();
        let mut fx = FxUnit::new(sr);
        fx.set_params(params);
        let noise_gain = measure_phaser_noise_rms_gain(&mut fx, sr, 0xFACE_0001);
        assert!(
            noise_gain < 0.82,
            "colored noise should lose clear energy in moving notches, gain={noise_gain}"
        );

        let rate_hz = PHASER_RATE_HZ_MIN + params.phaser_rate * PHASER_RATE_HZ_SPAN;
        let mut min_tone_gain = 1.0f32;
        for step in 0..32 {
            let mut fx = FxUnit::new(sr);
            fx.set_params(params);
            let adv = ((step as f32 / 32.0) * sr / rate_hz.max(0.05)) as usize;
            if adv > 0 {
                let mut z = vec![0.0f32; adv.min(12_000)];
                fx.process(&mut z);
            }
            for freq in (100..3100).step_by(80).map(|f| f as f32) {
                let g = measure_phaser_tone_gain_at_lfo(&mut fx, freq, sr);
                min_tone_gain = min_tone_gain.min(g);
            }
        }
        assert!(
            min_tone_gain < 0.82,
            "expected audible combing while the LFO sweeps, worst gain={min_tone_gain}"
        );
    }

    #[test]
    fn phaser_notches_move_when_lfo_advances() {
        let sr = 48_000.0;
        let params = phaser_params_loud();
        let rate_hz = PHASER_RATE_HZ_MIN + params.phaser_rate * PHASER_RATE_HZ_SPAN;

        let mut fx = FxUnit::new(sr);
        fx.set_params(params);
        let g_600 = measure_phaser_tone_gain_at_lfo(&mut fx, 600.0, sr);

        let half_period = (0.5 / rate_hz * sr) as usize;
        let mut silence = vec![0.0f32; half_period.max(512)];
        fx.process(&mut silence);

        let g_600_late = measure_phaser_tone_gain_at_lfo(&mut fx, 600.0, sr);

        let mut fx3 = FxUnit::new(sr);
        fx3.set_params(params);
        let noise_early = measure_phaser_noise_rms_gain(&mut fx3, sr, 0xA5A5_1234);
        let half = (0.5 / rate_hz.max(0.05) * sr) as usize;
        let mut z = vec![0.0f32; half.max(256)];
        fx3.process(&mut z);
        let noise_late = measure_phaser_noise_rms_gain(&mut fx3, sr, 0xA5A5_1234);
        assert!(
            (g_600 - g_600_late).abs() > 0.02
                || (noise_early - noise_late).abs() > 0.03,
            "gain@600 early={g_600} late={g_600_late} noise early={noise_early} late={noise_late}"
        );
    }

    #[test]
    fn phaser_wet_alters_a_tone() {
        let mut fx = FxUnit::new(48_000.0);
        fx.set_params(phaser_params_loud());
        let mut buf = vec![0.0f32; 512];
        for (i, s) in buf.iter_mut().enumerate() {
            *s = (i as f32 * 0.08).sin() * 0.5;
        }
        let before = buf[100];
        fx.process(&mut buf);
        assert!((buf[100] - before).abs() > 1e-3);
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
