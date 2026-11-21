//! SEQ mode loop-start click: a short sine on the master bus, outside the kit.

const CLICK_SEC: f32 = 0.045;
const FREQ_ACCENT: f32 = 1_200.0;
const FREQ_WEAK: f32 = 880.0;
const AMP_ACCENT: f32 = 0.28;
const AMP_WEAK: f32 = 0.18;

pub struct SeqCueClick {
    sample_rate: f32,
    len: u32,
    active: bool,
    pos: u32,
    phase: f64,
    freq: f32,
    amp: f32,
    /// Sample index within the current render block where the click starts.
    start_in_block: u32,
}

impl SeqCueClick {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8_000.0);
        Self {
            sample_rate: sr,
            len: (CLICK_SEC * sr).round().max(1.0) as u32,
            active: false,
            pos: 0,
            phase: 0.0,
            freq: FREQ_ACCENT,
            amp: AMP_ACCENT,
            start_in_block: 0,
        }
    }

    pub fn trigger_in_block(&mut self, frame_in_block: u32, accent: bool) {
        self.active = true;
        self.pos = 0;
        self.phase = 0.0;
        self.start_in_block = frame_in_block;
        self.freq = if accent { FREQ_ACCENT } else { FREQ_WEAK };
        self.amp = if accent { AMP_ACCENT } else { AMP_WEAK };
    }

    /// Add the click into `out` for samples `[span_off .. span_off + out.len())` of the block.
    pub fn mix_span(&mut self, out: &mut [f32], span_off: usize) {
        if !self.active || out.is_empty() {
            return;
        }
        let sr = self.sample_rate;
        let len = self.len;
        for (i, sample) in out.iter_mut().enumerate() {
            let block_i = span_off + i;
            if (block_i as u32) < self.start_in_block {
                continue;
            }
            if self.pos >= len {
                self.active = false;
                break;
            }
            let env = (1.0 - self.pos as f32 / len as f32).powi(2);
            *sample += (self.phase.sin() as f32) * env * self.amp;
            self.phase += std::f64::consts::TAU * self.freq as f64 / sr as f64;
            self.pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_is_short_and_adds_energy() {
        let mut click = SeqCueClick::new(48_000.0);
        click.trigger_in_block(0, true);
        let mut buf = [0.0f32; 512];
        click.mix_span(&mut buf, 0);
        let e: f32 = buf.iter().map(|s| s * s).sum();
        assert!(e > 0.001, "accent click should add audible energy, e={e}");
        assert!(click.active, "45ms click should still be sounding in a 512-sample block");
    }
}