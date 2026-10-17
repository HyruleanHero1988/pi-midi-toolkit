//! Lock-free master-mix tap and a triggered min/max scope frame.
//!
//! The audio callback only stores samples. The control thread builds one
//! picture per UI request: a rising-zero trigger when the pitch is steady,
//! and the min and max of every sample in each column so a peak cannot fall
//! between pixels.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use jambox_protocol::{ScopeReply, SCOPE_COLUMNS, WAVE_VIZ_POINTS};

/// Samples kept for the scope. At 48 kHz this is about 85 ms, enough for two
/// cycles of a low bass note.
const RING: usize = 4096;
const RING_MASK: u32 = (RING as u32) - 1;

/// Shared between the audio callback and the control thread.
pub struct ScopeTap {
    samples: Box<[AtomicU32]>,
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
    sub: Box<[AtomicU32]>,
    write: AtomicU32,
    rate: AtomicU32,
    /// Crossover in millihertz. The sub is low-passed here and left/right are
    /// high-passed here, so a drum hit stays in the sub wave. Default 80 Hz.
    sub_hz_milli: AtomicU32,
    coeff_stamp: AtomicU32,
    b0: AtomicU32,
    b1: AtomicU32,
    b2: AtomicU32,
    a1: AtomicU32,
    a2: AtomicU32,
    x1: AtomicU32,
    x2: AtomicU32,
    y1: AtomicU32,
    y2: AtomicU32,
    hp_b0: AtomicU32,
    hp_b1: AtomicU32,
    hp_b2: AtomicU32,
    hp_a1: AtomicU32,
    hp_a2: AtomicU32,
    lx1: AtomicU32,
    lx2: AtomicU32,
    ly1: AtomicU32,
    ly2: AtomicU32,
    rx1: AtomicU32,
    rx2: AtomicU32,
    ry1: AtomicU32,
    ry2: AtomicU32,
    /// Second Butterworth stage so the split is 24 dB/octave.
    lp2_x1: AtomicU32,
    lp2_x2: AtomicU32,
    lp2_y1: AtomicU32,
    lp2_y2: AtomicU32,
    l2_x1: AtomicU32,
    l2_x2: AtomicU32,
    l2_y1: AtomicU32,
    l2_y2: AtomicU32,
    r2_x1: AtomicU32,
    r2_x2: AtomicU32,
    r2_y1: AtomicU32,
    r2_y2: AtomicU32,
    /// Last accepted period, in samples. Zero means "not locked yet".
    period: AtomicU32,
    /// Trigger position as samples back from the end of the snapshot, modulo the period.
    phase: AtomicU32,
    pending: AtomicU32,
    pending_n: AtomicU32,
}

struct ScopeMemory {
    period: u32,
    phase: u32,
    pending: u32,
    pending_n: u32,
}

impl ScopeTap {
    pub fn new() -> Arc<Self> {
        let ring = || -> Box<[AtomicU32]> { (0..RING).map(|_| AtomicU32::new(0)).collect() };
        Arc::new(Self {
            samples: ring(),
            left: ring(),
            right: ring(),
            sub: ring(),
            write: AtomicU32::new(0),
            rate: AtomicU32::new(48_000),
            sub_hz_milli: AtomicU32::new(80_000),
            coeff_stamp: AtomicU32::new(0),
            b0: AtomicU32::new(0),
            b1: AtomicU32::new(0),
            b2: AtomicU32::new(0),
            a1: AtomicU32::new(0),
            a2: AtomicU32::new(0),
            x1: AtomicU32::new(0),
            x2: AtomicU32::new(0),
            y1: AtomicU32::new(0),
            y2: AtomicU32::new(0),
            hp_b0: AtomicU32::new(0),
            hp_b1: AtomicU32::new(0),
            hp_b2: AtomicU32::new(0),
            hp_a1: AtomicU32::new(0),
            hp_a2: AtomicU32::new(0),
            lx1: AtomicU32::new(0),
            lx2: AtomicU32::new(0),
            ly1: AtomicU32::new(0),
            ly2: AtomicU32::new(0),
            rx1: AtomicU32::new(0),
            rx2: AtomicU32::new(0),
            ry1: AtomicU32::new(0),
            ry2: AtomicU32::new(0),
            lp2_x1: AtomicU32::new(0),
            lp2_x2: AtomicU32::new(0),
            lp2_y1: AtomicU32::new(0),
            lp2_y2: AtomicU32::new(0),
            l2_x1: AtomicU32::new(0),
            l2_x2: AtomicU32::new(0),
            l2_y1: AtomicU32::new(0),
            l2_y2: AtomicU32::new(0),
            r2_x1: AtomicU32::new(0),
            r2_x2: AtomicU32::new(0),
            r2_y1: AtomicU32::new(0),
            r2_y2: AtomicU32::new(0),
            period: AtomicU32::new(0),
            phase: AtomicU32::new(u32::MAX),
            pending: AtomicU32::new(0),
            pending_n: AtomicU32::new(0),
        })
    }

    /// Crossover in Hz. Sub keeps what is below it; left and right keep what
    /// is above it. Clamped to a subwoofer range.
    pub fn set_sub_hz(&self, hz: f32) {
        let milli = (hz.clamp(20.0, 200.0) * 1000.0).round() as u32;
        self.sub_hz_milli.store(milli.max(1), Ordering::Relaxed);
    }

    pub fn set_rate(&self, rate: u32) {
        if rate > 0 {
            self.rate.store(rate, Ordering::Relaxed);
        }
    }

    /// Audio thread. The device bus is dual-mono, so both wave channels
    /// receive this block and the sub is its low end.
    pub fn push_block(&self, block: &[f32]) {
        self.push_channels(block, block);
    }

    /// Audio thread. Stores a low-passed sum and high-passed left/right, then
    /// publishes the write index. The existing scope picture uses the full
    /// summed bus.
    pub fn push_channels(&self, left: &[f32], right: &[f32]) {
        let n = left.len().min(right.len());
        if n == 0 {
            return;
        }
        let (b0, b1, b2, a1, a2, hb0, hb1, hb2, ha1, ha2) = self.sub_coeffs();
        let mut x1 = f32::from_bits(self.x1.load(Ordering::Relaxed));
        let mut x2 = f32::from_bits(self.x2.load(Ordering::Relaxed));
        let mut y1 = f32::from_bits(self.y1.load(Ordering::Relaxed));
        let mut y2 = f32::from_bits(self.y2.load(Ordering::Relaxed));
        let mut lx1 = f32::from_bits(self.lx1.load(Ordering::Relaxed));
        let mut lx2 = f32::from_bits(self.lx2.load(Ordering::Relaxed));
        let mut ly1 = f32::from_bits(self.ly1.load(Ordering::Relaxed));
        let mut ly2 = f32::from_bits(self.ly2.load(Ordering::Relaxed));
        let mut rx1 = f32::from_bits(self.rx1.load(Ordering::Relaxed));
        let mut rx2 = f32::from_bits(self.rx2.load(Ordering::Relaxed));
        let mut ry1 = f32::from_bits(self.ry1.load(Ordering::Relaxed));
        let mut ry2 = f32::from_bits(self.ry2.load(Ordering::Relaxed));
        let mut lp2_x1 = f32::from_bits(self.lp2_x1.load(Ordering::Relaxed));
        let mut lp2_x2 = f32::from_bits(self.lp2_x2.load(Ordering::Relaxed));
        let mut lp2_y1 = f32::from_bits(self.lp2_y1.load(Ordering::Relaxed));
        let mut lp2_y2 = f32::from_bits(self.lp2_y2.load(Ordering::Relaxed));
        let mut l2_x1 = f32::from_bits(self.l2_x1.load(Ordering::Relaxed));
        let mut l2_x2 = f32::from_bits(self.l2_x2.load(Ordering::Relaxed));
        let mut l2_y1 = f32::from_bits(self.l2_y1.load(Ordering::Relaxed));
        let mut l2_y2 = f32::from_bits(self.l2_y2.load(Ordering::Relaxed));
        let mut r2_x1 = f32::from_bits(self.r2_x1.load(Ordering::Relaxed));
        let mut r2_x2 = f32::from_bits(self.r2_x2.load(Ordering::Relaxed));
        let mut r2_y1 = f32::from_bits(self.r2_y1.load(Ordering::Relaxed));
        let mut r2_y2 = f32::from_bits(self.r2_y2.load(Ordering::Relaxed));
        let mut w = self.write.load(Ordering::Relaxed);
        for i in 0..n {
            let l = left[i];
            let r = right[i];
            // Half the sum so a dual-mono bus stays at the level of one channel.
            let mono = (l + r) * 0.5;
            let y = b0 * mono + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = mono;
            y2 = y1;
            y1 = y;
            let sub = b0 * y + b1 * lp2_x1 + b2 * lp2_x2 - a1 * lp2_y1 - a2 * lp2_y2;
            lp2_x2 = lp2_x1;
            lp2_x1 = y;
            lp2_y2 = lp2_y1;
            lp2_y1 = sub;
            let hl = hb0 * l + hb1 * lx1 + hb2 * lx2 - ha1 * ly1 - ha2 * ly2;
            lx2 = lx1;
            lx1 = l;
            ly2 = ly1;
            ly1 = hl;
            let left_hp = hb0 * hl + hb1 * l2_x1 + hb2 * l2_x2 - ha1 * l2_y1 - ha2 * l2_y2;
            l2_x2 = l2_x1;
            l2_x1 = hl;
            l2_y2 = l2_y1;
            l2_y1 = left_hp;
            let hr = hb0 * r + hb1 * rx1 + hb2 * rx2 - ha1 * ry1 - ha2 * ry2;
            rx2 = rx1;
            rx1 = r;
            ry2 = ry1;
            ry1 = hr;
            let right_hp = hb0 * hr + hb1 * r2_x1 + hb2 * r2_x2 - ha1 * r2_y1 - ha2 * r2_y2;
            r2_x2 = r2_x1;
            r2_x1 = hr;
            r2_y2 = r2_y1;
            r2_y1 = right_hp;
            let idx = (w & RING_MASK) as usize;
            self.samples[idx].store(mono.to_bits(), Ordering::Relaxed);
            self.left[idx].store(left_hp.to_bits(), Ordering::Relaxed);
            self.right[idx].store(right_hp.to_bits(), Ordering::Relaxed);
            self.sub[idx].store(sub.to_bits(), Ordering::Relaxed);
            w = w.wrapping_add(1);
        }
        self.x1.store(x1.to_bits(), Ordering::Relaxed);
        self.x2.store(x2.to_bits(), Ordering::Relaxed);
        self.y1.store(y1.to_bits(), Ordering::Relaxed);
        self.y2.store(y2.to_bits(), Ordering::Relaxed);
        self.lx1.store(lx1.to_bits(), Ordering::Relaxed);
        self.lx2.store(lx2.to_bits(), Ordering::Relaxed);
        self.ly1.store(ly1.to_bits(), Ordering::Relaxed);
        self.ly2.store(ly2.to_bits(), Ordering::Relaxed);
        self.rx1.store(rx1.to_bits(), Ordering::Relaxed);
        self.rx2.store(rx2.to_bits(), Ordering::Relaxed);
        self.ry1.store(ry1.to_bits(), Ordering::Relaxed);
        self.ry2.store(ry2.to_bits(), Ordering::Relaxed);
        self.lp2_x1.store(lp2_x1.to_bits(), Ordering::Relaxed);
        self.lp2_x2.store(lp2_x2.to_bits(), Ordering::Relaxed);
        self.lp2_y1.store(lp2_y1.to_bits(), Ordering::Relaxed);
        self.lp2_y2.store(lp2_y2.to_bits(), Ordering::Relaxed);
        self.l2_x1.store(l2_x1.to_bits(), Ordering::Relaxed);
        self.l2_x2.store(l2_x2.to_bits(), Ordering::Relaxed);
        self.l2_y1.store(l2_y1.to_bits(), Ordering::Relaxed);
        self.l2_y2.store(l2_y2.to_bits(), Ordering::Relaxed);
        self.r2_x1.store(r2_x1.to_bits(), Ordering::Relaxed);
        self.r2_x2.store(r2_x2.to_bits(), Ordering::Relaxed);
        self.r2_y1.store(r2_y1.to_bits(), Ordering::Relaxed);
        self.r2_y2.store(r2_y2.to_bits(), Ordering::Relaxed);
        self.write.store(w, Ordering::Release);
    }

    fn sub_coeffs(&self) -> (f32, f32, f32, f32, f32, f32, f32, f32, f32, f32) {
        let rate = self.rate.load(Ordering::Relaxed).max(1);
        let milli = self.sub_hz_milli.load(Ordering::Relaxed).max(1);
        let stamp = rate ^ milli.rotate_left(1);
        if stamp != self.coeff_stamp.load(Ordering::Relaxed) {
            let hz = milli as f32 / 1000.0;
            let (b0, b1, b2, a1, a2) = design_lowpass(rate as f32, hz);
            let (hb0, hb1, hb2, ha1, ha2) = design_highpass(rate as f32, hz);
            self.b0.store(b0.to_bits(), Ordering::Relaxed);
            self.b1.store(b1.to_bits(), Ordering::Relaxed);
            self.b2.store(b2.to_bits(), Ordering::Relaxed);
            self.a1.store(a1.to_bits(), Ordering::Relaxed);
            self.a2.store(a2.to_bits(), Ordering::Relaxed);
            self.hp_b0.store(hb0.to_bits(), Ordering::Relaxed);
            self.hp_b1.store(hb1.to_bits(), Ordering::Relaxed);
            self.hp_b2.store(hb2.to_bits(), Ordering::Relaxed);
            self.hp_a1.store(ha1.to_bits(), Ordering::Relaxed);
            self.hp_a2.store(ha2.to_bits(), Ordering::Relaxed);
            self.coeff_stamp.store(stamp, Ordering::Relaxed);
        }
        (
            f32::from_bits(self.b0.load(Ordering::Relaxed)),
            f32::from_bits(self.b1.load(Ordering::Relaxed)),
            f32::from_bits(self.b2.load(Ordering::Relaxed)),
            f32::from_bits(self.a1.load(Ordering::Relaxed)),
            f32::from_bits(self.a2.load(Ordering::Relaxed)),
            f32::from_bits(self.hp_b0.load(Ordering::Relaxed)),
            f32::from_bits(self.hp_b1.load(Ordering::Relaxed)),
            f32::from_bits(self.hp_b2.load(Ordering::Relaxed)),
            f32::from_bits(self.hp_a1.load(Ordering::Relaxed)),
            f32::from_bits(self.hp_a2.load(Ordering::Relaxed)),
        )
    }

    /// Newest samples, oldest first. `out` should be [`RING`] long.
    #[cfg(test)]
    pub fn snapshot(&self, out: &mut [f32]) -> (usize, u32) {
        let write = self.write.load(Ordering::Acquire);
        self.copy_from(write, out)
    }

    fn copy_from(&self, write: u32, out: &mut [f32]) -> (usize, u32) {
        self.copy_ring(&self.samples, write, out)
    }

    fn copy_ring(&self, ring: &[AtomicU32], write: u32, out: &mut [f32]) -> (usize, u32) {
        let available = (write as usize).min(RING).min(out.len());
        let start = write.wrapping_sub(available as u32);
        for i in 0..available {
            let idx = start.wrapping_add(i as u32) & RING_MASK;
            out[i] = f32::from_bits(ring[idx as usize].load(Ordering::Relaxed));
        }
        (available, self.rate.load(Ordering::Relaxed))
    }

    /// Scope picture, plus left / right / sub traces when `wave` is set.
    pub fn frame_with_wave(&self, wave: bool) -> ScopeReply {
        let mut raw = [0.0f32; RING];
        let write = self.write.load(Ordering::Acquire);
        let (n, rate) = self.copy_from(write, &mut raw);
        let mut mem = ScopeMemory {
            period: self.period.load(Ordering::Relaxed),
            phase: self.phase.load(Ordering::Relaxed),
            pending: self.pending.load(Ordering::Relaxed),
            pending_n: self.pending_n.load(Ordering::Relaxed),
        };
        let mut reply = build_tracked(&raw[..n], rate, write, &mut mem);
        self.period.store(mem.period, Ordering::Relaxed);
        self.phase.store(mem.phase, Ordering::Relaxed);
        self.pending.store(mem.pending, Ordering::Relaxed);
        self.pending_n.store(mem.pending_n, Ordering::Relaxed);
        if wave {
            let mut raw_l = [0.0f32; RING];
            let mut raw_r = [0.0f32; RING];
            let mut raw_s = [0.0f32; RING];
            let (nl, _) = self.copy_ring(&self.left, write, &mut raw_l);
            let (nr, _) = self.copy_ring(&self.right, write, &mut raw_r);
            let (ns, _) = self.copy_ring(&self.sub, write, &mut raw_s);
            reply.left = decimate_line(&raw_l[..nl], WAVE_VIZ_POINTS);
            reply.right = decimate_line(&raw_r[..nr], WAVE_VIZ_POINTS);
            reply.sub = decimate_line(&raw_s[..ns], WAVE_VIZ_POINTS);
        }
        reply
    }
}

/// Butterworth low-pass, normalized so `a0` is 1.
fn design_lowpass(sample_rate: f32, cutoff_hz: f32) -> (f32, f32, f32, f32, f32) {
    let fs = sample_rate.max(1.0);
    let fc = cutoff_hz.clamp(10.0, fs * 0.45);
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let w0 = std::f32::consts::TAU * fc / fs;
    let cos = w0.cos();
    let sin = w0.sin();
    let alpha = sin / (2.0 * q);
    let b0 = (1.0 - cos) * 0.5;
    let b1 = 1.0 - cos;
    let b2 = (1.0 - cos) * 0.5;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos;
    let a2 = 1.0 - alpha;
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

/// Butterworth high-pass at the same cutoff as [`design_lowpass`].
fn design_highpass(sample_rate: f32, cutoff_hz: f32) -> (f32, f32, f32, f32, f32) {
    let fs = sample_rate.max(1.0);
    let fc = cutoff_hz.clamp(10.0, fs * 0.45);
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let w0 = std::f32::consts::TAU * fc / fs;
    let cos = w0.cos();
    let sin = w0.sin();
    let alpha = sin / (2.0 * q);
    let b0 = (1.0 + cos) * 0.5;
    let b1 = -(1.0 + cos);
    let b2 = (1.0 + cos) * 0.5;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos;
    let a2 = 1.0 - alpha;
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

fn decimate_line(samples: &[f32], points: usize) -> Vec<i16> {
    let mut out = vec![0i16; points];
    if samples.is_empty() || points == 0 {
        return out;
    }
    if samples.len() == 1 || points == 1 {
        out.fill(quantize(samples[0]));
        return out;
    }
    let last = samples.len() - 1;
    let denom = (points - 1) as f32;
    for (i, slot) in out.iter_mut().enumerate() {
        let idx = ((i as f32 / denom) * last as f32).round() as usize;
        *slot = quantize(samples[idx.min(last)]);
    }
    out
}

/// Build one scope picture from a chronological sample window.
pub fn build(samples: &[f32], sample_rate: u32) -> ScopeReply {
    let mut mem = ScopeMemory {
        period: 0,
        phase: u32::MAX,
        pending: 0,
        pending_n: 0,
    };
    build_tracked(samples, sample_rate, samples.len() as u32, &mut mem)
}

/// Same picture as [`build`], but the period and trigger phase stick across
/// frames. A ripple that adds one extra edge used to drop the lock and jump
/// the zoom from two cycles to a long window, which reads as two different waves.
///
/// `end_index` is the absolute sample count at the end of `samples`. The trigger
/// is remembered in that count, not as "samples back from the end" — that offset
/// walks as audio arrives, and a half-cycle step locks the opposite lobe, which
/// draws the wave upside down.
fn build_tracked(
    samples: &[f32],
    sample_rate: u32,
    end_index: u32,
    mem: &mut ScopeMemory,
) -> ScopeReply {
    let mut reply = ScopeReply {
        rate: sample_rate,
        ..ScopeReply::default()
    };
    if samples.len() < 32 || sample_rate == 0 {
        reply.min = vec![0; SCOPE_COLUMNS];
        reply.max = vec![0; SCOPE_COLUMNS];
        return reply;
    }
    let mean = samples.iter().copied().sum::<f32>() / samples.len() as f32;
    let min_win = (sample_rate as usize / 800).max(32);
    let max_win = (sample_rate as usize / 20).min(samples.len());
    let candidate = estimate_period(samples, mean);
    let period = track_period(candidate, mem);
    let window = match period {
        Some(period) => period.saturating_mul(2).clamp(min_win, max_win),
        None => (sample_rate as usize / 40).clamp(min_win, max_win),
    };
    let window = window.min(samples.len());
    let start = trigger_start(samples, mean, window, period, end_index, mem);
    let mut min = vec![0i16; SCOPE_COLUMNS];
    let mut max = vec![0i16; SCOPE_COLUMNS];
    decimate(&samples[start..start + window], &mut min, &mut max);
    reply.min = min;
    reply.max = max;
    reply
}

/// Schmitt rising edges. A wiggle around the mean does not count as a new cycle.
fn rising_edges(samples: &[f32], mean: f32, out: &mut [usize]) -> usize {
    let mut peak = 0.0f32;
    for sample in samples {
        peak = peak.max((sample - mean).abs());
    }
    if peak < 0.015 {
        return 0;
    }
    let hyst = peak * 0.12;
    let mut armed = false;
    let mut n = 0usize;
    for (i, sample) in samples.iter().enumerate() {
        let centered = sample - mean;
        if centered < -hyst {
            armed = true;
        } else if armed && centered > hyst {
            armed = false;
            if n == out.len() {
                out.copy_within(1.., 0);
                n -= 1;
            }
            out[n] = i;
            n += 1;
        }
    }
    n
}

fn estimate_period(samples: &[f32], mean: f32) -> Option<usize> {
    let mut edges = [0usize; 48];
    let n = rising_edges(samples, mean, &mut edges);
    if n < 4 {
        return None;
    }
    let mut deltas = [0usize; 16];
    let take = (n - 1).min(deltas.len());
    for i in 0..take {
        deltas[i] = edges[n - take + i].saturating_sub(edges[n - take + i - 1]);
    }
    if let Some(period) = stable_median(&deltas[..take]) {
        return Some(period);
    }
    // Two lobes per cycle: the gaps alternate, and the real period is their sum.
    let pairs = take / 2;
    if pairs < 3 {
        return None;
    }
    let mut summed = [0usize; 8];
    for i in 0..pairs {
        summed[i] = deltas[i * 2].saturating_add(deltas[i * 2 + 1]);
    }
    stable_median(&summed[..pairs])
}

fn stable_median(deltas: &[usize]) -> Option<usize> {
    if deltas.len() < 3 {
        return None;
    }
    let mut sorted = [0usize; 16];
    let n = deltas.len().min(sorted.len());
    sorted[..n].copy_from_slice(&deltas[..n]);
    sorted[..n].sort_unstable();
    let median = sorted[n / 2];
    if median < 8 {
        return None;
    }
    let lo = median * 85 / 100;
    let hi = median.saturating_mul(118) / 100;
    if deltas[..n].iter().all(|d| *d >= lo && *d <= hi) {
        Some(median)
    } else {
        None
    }
}

/// Keep the zoom that is already on screen until a new pitch has held still.
/// An octave-sized flip that lasts one frame is how the trace was swapping shapes.
fn track_period(candidate: Option<usize>, mem: &mut ScopeMemory) -> Option<usize> {
    let Some(candidate) = candidate else {
        return (mem.period > 0).then_some(mem.period as usize);
    };
    if mem.period == 0 {
        mem.period = candidate as u32;
        mem.pending_n = 0;
        return Some(candidate);
    }
    let prev = mem.period as usize;
    let ratio = candidate as f32 / prev as f32;
    if (0.90..=1.10).contains(&ratio) {
        let blended = (prev as f32 * 0.75 + candidate as f32 * 0.25).round() as usize;
        mem.period = blended.max(8) as u32;
        mem.pending_n = 0;
        return Some(mem.period as usize);
    }
    let pending = mem.pending as usize;
    let agrees = pending > 0 && candidate.abs_diff(pending) * 8 < pending.max(1);
    if agrees {
        mem.pending_n = mem.pending_n.saturating_add(1);
        mem.pending = ((pending as f32) * 0.5 + candidate as f32 * 0.5).round() as u32;
    } else {
        mem.pending = candidate as u32;
        mem.pending_n = 1;
    }
    if mem.pending_n >= 6 {
        mem.period = mem.pending;
        mem.pending_n = 0;
        return Some(mem.period as usize);
    }
    Some(prev)
}

fn trigger_start(
    samples: &[f32],
    mean: f32,
    window: usize,
    period: Option<usize>,
    end_index: u32,
    mem: &mut ScopeMemory,
) -> usize {
    let last = samples.len() - window;
    let mut edges = [0usize; 48];
    let n = rising_edges(samples, mean, &mut edges);
    let mut usable = 0usize;
    for i in 0..n {
        if edges[i] <= last {
            edges[usable] = edges[i];
            usable += 1;
        }
    }
    if usable == 0 {
        return last;
    }
    let Some(period) = period.filter(|p| *p >= 8) else {
        let edge = edges[usable - 1];
        mem.phase = edge_abs(end_index, samples.len(), edge);
        return edge;
    };
    if mem.phase == u32::MAX {
        let edge = edges[usable - 1];
        mem.phase = edge_abs(end_index, samples.len(), edge);
        return edge;
    }
    let mut best = edges[usable - 1];
    let mut best_dist = usize::MAX;
    for edge in &edges[..usable] {
        let delta = edge_abs(end_index, samples.len(), *edge).wrapping_sub(mem.phase);
        let dist = circular_dist(delta as usize % period, 0, period);
        if dist < best_dist {
            best_dist = dist;
            best = *edge;
        }
    }
    // The other lobe sits half a period away. Taking it draws the wave upside down.
    let limit = (period / 5).max(2);
    if best_dist > limit {
        return hold_phase(samples.len(), end_index, mem.phase, period, last);
    }
    mem.phase = edge_abs(end_index, samples.len(), best);
    best
}

fn edge_abs(end_index: u32, len: usize, edge: usize) -> u32 {
    end_index.wrapping_sub(len as u32).wrapping_add(edge as u32)
}

/// Window start whose absolute index is the same phase as the last trigger.
fn hold_phase(len: usize, end_index: u32, trigger_abs: u32, period: usize, last: usize) -> usize {
    let start_abs = end_index.wrapping_sub(len as u32);
    let phase = trigger_abs.wrapping_sub(start_abs) as usize % period;
    if phase > last {
        let back = phase - last;
        let steps = back.div_ceil(period);
        return phase.saturating_sub(steps * period);
    }
    let steps = (last - phase) / period;
    phase + steps * period
}

fn circular_dist(a: usize, b: usize, period: usize) -> usize {
    let d = a.abs_diff(b) % period;
    d.min(period - d)
}

fn decimate(window: &[f32], min_out: &mut [i16], max_out: &mut [i16]) {
    let cols = min_out.len().min(max_out.len());
    let n = window.len().max(1);
    for col in 0..cols {
        let a = col * n / cols;
        let b = ((col + 1) * n / cols).max(a + 1).min(n);
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for sample in &window[a..b] {
            lo = lo.min(*sample);
            hi = hi.max(*sample);
        }
        min_out[col] = quantize(lo);
        max_out[col] = quantize(hi);
    }
}

fn quantize(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_locks_and_reaches_both_rails() {
        let rate = 48_000u32;
        let mut samples = [0.0f32; RING];
        for (i, sample) in samples.iter_mut().enumerate() {
            let t = i as f32 / rate as f32;
            *sample = (t * 440.0 * std::f32::consts::TAU).sin() * 0.8;
        }
        let frame = build(&samples, rate);
        let peak = frame.max.iter().copied().max().unwrap_or(0);
        let trough = frame.min.iter().copied().min().unwrap_or(0);
        assert!(peak > 24_000, "peak {peak}");
        assert!(trough < -24_000, "trough {trough}");
    }

    #[test]
    fn a_step_keeps_both_extremes_in_one_column() {
        let mut samples = [0.0f32; RING];
        for (i, sample) in samples.iter_mut().enumerate() {
            *sample = if (i / 50) % 2 == 0 { 0.9 } else { -0.9 };
        }
        let frame = build(&samples, 48_000);
        // A bin is either entirely high, entirely low, or the step itself.
        // Nothing is averaged toward zero.
        let faithful = frame.min.iter().zip(frame.max.iter()).all(|(lo, hi)| {
            let edge = *lo < -20_000 && *hi > 20_000;
            let high = *lo > 20_000 && *hi > 20_000;
            let low = *lo < -20_000 && *hi < -20_000;
            edge || high || low
        });
        assert!(faithful, "a column lost the square's rails");
    }

    #[test]
    fn silence_stays_on_the_center_line() {
        let frame = build(&[0.0; 1000], 48_000);
        assert!(frame.min.iter().all(|s| s.abs() < 10));
        assert!(frame.max.iter().all(|s| s.abs() < 10));
    }

    #[test]
    fn a_small_ripple_does_not_change_the_locked_period() {
        let rate = 48_000u32;
        let period = rate as f32 / 220.0;
        let mut clean = [0.0f32; RING];
        let mut rippled = [0.0f32; RING];
        for i in 0..RING {
            let t = i as f32 / rate as f32;
            let fundamental = (t * 220.0 * std::f32::consts::TAU).sin() * 0.8;
            clean[i] = fundamental;
            rippled[i] = fundamental + (t * 2200.0 * std::f32::consts::TAU).sin() * 0.04;
        }
        let mut mem = ScopeMemory {
            period: 0,
            phase: u32::MAX,
            pending: 0,
            pending_n: 0,
        };
        let _ = build_tracked(&clean, rate, clean.len() as u32, &mut mem);
        let locked = mem.period;
        assert!(
            (locked as f32 - period).abs() < period * 0.08,
            "locked {locked}, expected about {period}"
        );
        // One extra zero crossing used to drop the lock and jump the zoom.
        for _ in 0..4 {
            let _ = build_tracked(&rippled, rate, rippled.len() as u32, &mut mem);
        }
        assert!(
            (mem.period as f32 - period).abs() < period * 0.08,
            "ripple moved the period to {}",
            mem.period
        );
    }

    #[test]
    fn advancing_the_same_tone_keeps_the_same_picture() {
        let rate = 48_000u32;
        let mut mem = ScopeMemory {
            period: 0,
            phase: u32::MAX,
            pending: 0,
            pending_n: 0,
        };
        let mut samples = [0.0f32; RING + 200];
        for (i, sample) in samples.iter_mut().enumerate() {
            let t = i as f32 / rate as f32;
            *sample = (t * 330.0 * std::f32::consts::TAU).sin() * 0.75;
        }
        let first = build_tracked(&samples[..RING], rate, RING as u32, &mut mem);
        let second = build_tracked(&samples[200..], rate, (RING + 200) as u32, &mut mem);
        let peak_a = first
            .max
            .iter()
            .position(|s| *s == first.max.iter().copied().max().unwrap());
        let peak_b = second
            .max
            .iter()
            .position(|s| *s == second.max.iter().copied().max().unwrap());
        let shift = peak_a.unwrap().abs_diff(peak_b.unwrap());
        assert!(shift < 8, "picture slid by {shift} columns");
    }

    #[test]
    fn a_half_cycle_step_does_not_mirror_the_wave() {
        let rate = 48_000u32;
        let freq = 220.0f32;
        let half = (rate as f32 / freq / 2.0).round() as usize;
        let mut samples = vec![0.0f32; RING + half];
        for (i, sample) in samples.iter_mut().enumerate() {
            let t = i as f32 / rate as f32;
            *sample = (t * freq * std::f32::consts::TAU).sin() * 0.8;
        }
        let mut mem = ScopeMemory {
            period: 0,
            phase: u32::MAX,
            pending: 0,
            pending_n: 0,
        };
        let first = build_tracked(&samples[..RING], rate, RING as u32, &mut mem);
        let second = build_tracked(
            &samples[half..half + RING],
            rate,
            (RING + half) as u32,
            &mut mem,
        );
        let corr: f32 = first
            .max
            .iter()
            .zip(first.min.iter())
            .zip(second.max.iter().zip(second.min.iter()))
            .map(|((a_hi, a_lo), (b_hi, b_lo))| {
                let a = (*a_hi as f32 + *a_lo as f32) * 0.5;
                let b = (*b_hi as f32 + *b_lo as f32) * 0.5;
                a * b
            })
            .sum();
        assert!(
            corr > 0.0,
            "half-cycle advance mirrored the trace, corr={corr}"
        );
    }

    #[test]
    fn tap_publishes_a_block_in_order() {
        let tap = ScopeTap::new();
        tap.set_rate(44_100);
        let block: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        tap.push_block(&block);
        let mut out = [0.0f32; RING];
        let (n, rate) = tap.snapshot(&mut out);
        assert_eq!(rate, 44_100);
        assert_eq!(n, 100);
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[99] - 0.99).abs() < 1e-6);
    }

    fn tone(rate: u32, hz: f32, amp: f32) -> Vec<f32> {
        (0..rate as usize)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (t * hz * std::f32::consts::TAU).sin() * amp
            })
            .collect()
    }

    fn push_pair(tap: &ScopeTap, left: &[f32], right: &[f32]) {
        for (l, r) in left.chunks(256).zip(right.chunks(256)) {
            tap.push_channels(l, r);
        }
    }

    #[test]
    fn channels_stay_separate_and_sub_drops_highs() {
        let rate = 48_000u32;
        let bass = tone(rate, 40.0, 0.6);
        let high = tone(rate, 2000.0, 0.6);
        let silent = vec![0.0f32; bass.len()];

        let split = ScopeTap::new();
        split.set_rate(rate);
        split.set_sub_hz(80.0);
        push_pair(&split, &bass, &high);
        let frame = split.frame_with_wave(true);
        assert_eq!(frame.left.len(), jambox_protocol::WAVE_VIZ_POINTS);
        assert!(
            frame
                .left
                .iter()
                .zip(frame.right.iter())
                .any(|(l, r)| l.abs_diff(*r) > 8_000),
            "left and right collapsed into one trace"
        );

        let low_only = ScopeTap::new();
        low_only.set_rate(rate);
        low_only.set_sub_hz(80.0);
        push_pair(&low_only, &bass, &silent);
        let high_only = ScopeTap::new();
        high_only.set_rate(rate);
        high_only.set_sub_hz(80.0);
        push_pair(&high_only, &high, &silent);
        let peak = |samples: &[i16]| samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        let low_frame = low_only.frame_with_wave(true);
        let high_frame = high_only.frame_with_wave(true);
        let bass_peak = peak(&low_frame.sub);
        let high_peak = peak(&high_frame.sub);
        assert!(bass_peak > 8_000, "sub lost the bass, peak {bass_peak}");
        assert!(
            high_peak * 8 < bass_peak,
            "sub kept the highs, bass {bass_peak} high {high_peak}"
        );
        let bass_side = peak(&low_frame.left);
        let high_side = peak(&high_frame.left);
        assert!(high_side > 8_000, "left lost the highs, peak {high_side}");
        assert!(
            bass_side * 4 < high_side,
            "left kept the drum thump, bass {bass_side} high {high_side}"
        );
    }

    #[test]
    fn raising_the_split_pulls_bass_off_the_sides() {
        let rate = 48_000u32;
        let bass = tone(rate, 40.0, 0.8);
        let open = ScopeTap::new();
        open.set_rate(rate);
        open.set_sub_hz(20.0);
        push_pair(&open, &bass, &bass);
        let closed = ScopeTap::new();
        closed.set_rate(rate);
        closed.set_sub_hz(180.0);
        push_pair(&closed, &bass, &bass);
        let peak = |samples: &[i16]| samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        let open_side = peak(&open.frame_with_wave(true).left);
        let closed_side = peak(&closed.frame_with_wave(true).left);
        let closed_sub = peak(&closed.frame_with_wave(true).sub);
        assert!(
            open_side > 8_000,
            "a 40 Hz thump should stay on the sides when the split is 20 Hz, peak {open_side}"
        );
        assert!(
            closed_side * 4 < open_side,
            "split did not move the thump, open {open_side} closed {closed_side}"
        );
        assert!(
            closed_sub > 4_000,
            "sub lost the thump at 180 Hz, peak {closed_sub}"
        );
    }
}
