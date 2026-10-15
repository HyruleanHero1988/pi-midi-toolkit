//! Lock-free master-mix tap and a triggered min/max scope frame.
//!
//! The audio callback only stores samples. The control thread builds one
//! picture per UI request: a rising-zero trigger when the pitch is steady,
//! and the min and max of every sample in each column so a peak cannot fall
//! between pixels.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use jambox_protocol::{ScopeReply, SCOPE_COLUMNS};

/// Samples kept for the scope. At 48 kHz this is about 85 ms, enough for two
/// cycles of a low bass note.
const RING: usize = 4096;
const RING_MASK: u32 = (RING as u32) - 1;

/// Shared between the audio callback and the control thread.
pub struct ScopeTap {
    samples: Box<[AtomicU32]>,
    write: AtomicU32,
    rate: AtomicU32,
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
        let samples: Box<[AtomicU32]> = (0..RING).map(|_| AtomicU32::new(0)).collect();
        Arc::new(Self {
            samples,
            write: AtomicU32::new(0),
            rate: AtomicU32::new(48_000),
            period: AtomicU32::new(0),
            phase: AtomicU32::new(0),
            pending: AtomicU32::new(0),
            pending_n: AtomicU32::new(0),
        })
    }

    pub fn set_rate(&self, rate: u32) {
        if rate > 0 {
            self.rate.store(rate, Ordering::Relaxed);
        }
    }

    /// Audio thread. Stores the block, then publishes the write index.
    pub fn push_block(&self, block: &[f32]) {
        if block.is_empty() {
            return;
        }
        let mut w = self.write.load(Ordering::Relaxed);
        for &sample in block {
            self.samples[(w & RING_MASK) as usize].store(sample.to_bits(), Ordering::Relaxed);
            w = w.wrapping_add(1);
        }
        self.write.store(w, Ordering::Release);
    }

    /// Newest samples, oldest first. `out` should be [`RING`] long.
    pub fn snapshot(&self, out: &mut [f32]) -> (usize, u32) {
        let write = self.write.load(Ordering::Acquire);
        let available = (write as usize).min(RING).min(out.len());
        let start = write.wrapping_sub(available as u32);
        for i in 0..available {
            let idx = start.wrapping_add(i as u32) & RING_MASK;
            out[i] = f32::from_bits(self.samples[idx as usize].load(Ordering::Relaxed));
        }
        (available, self.rate.load(Ordering::Relaxed))
    }

    pub fn frame(&self) -> ScopeReply {
        let mut raw = [0.0f32; RING];
        let (n, rate) = self.snapshot(&mut raw);
        let mut mem = ScopeMemory {
            period: self.period.load(Ordering::Relaxed),
            phase: self.phase.load(Ordering::Relaxed),
            pending: self.pending.load(Ordering::Relaxed),
            pending_n: self.pending_n.load(Ordering::Relaxed),
        };
        let reply = build_tracked(&raw[..n], rate, &mut mem);
        self.period.store(mem.period, Ordering::Relaxed);
        self.phase.store(mem.phase, Ordering::Relaxed);
        self.pending.store(mem.pending, Ordering::Relaxed);
        self.pending_n.store(mem.pending_n, Ordering::Relaxed);
        reply
    }
}

/// Build one scope picture from a chronological sample window.
pub fn build(samples: &[f32], sample_rate: u32) -> ScopeReply {
    let mut mem = ScopeMemory {
        period: 0,
        phase: 0,
        pending: 0,
        pending_n: 0,
    };
    build_tracked(samples, sample_rate, &mut mem)
}

/// Same picture as [`build`], but the period and trigger phase stick across
/// frames. A ripple that adds one extra edge used to drop the lock and jump
/// the zoom from two cycles to a long window, which reads as two different waves.
fn build_tracked(samples: &[f32], sample_rate: u32, mem: &mut ScopeMemory) -> ScopeReply {
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
    let start = trigger_start(samples, mean, window, period, mem);
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
        return edges[usable - 1];
    };
    let want = mem.phase as usize % period;
    let mut best = edges[usable - 1];
    let mut best_dist = usize::MAX;
    for edge in &edges[..usable] {
        let phase = samples.len().saturating_sub(*edge) % period;
        let dist = circular_dist(phase, want, period);
        if dist < best_dist {
            best_dist = dist;
            best = *edge;
        }
    }
    mem.phase = (samples.len().saturating_sub(best) % period) as u32;
    best
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
            phase: 0,
            pending: 0,
            pending_n: 0,
        };
        let _ = build_tracked(&clean, rate, &mut mem);
        let locked = mem.period;
        assert!(
            (locked as f32 - period).abs() < period * 0.08,
            "locked {locked}, expected about {period}"
        );
        // One extra zero crossing used to drop the lock and jump the zoom.
        for _ in 0..4 {
            let _ = build_tracked(&rippled, rate, &mut mem);
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
            phase: 0,
            pending: 0,
            pending_n: 0,
        };
        let mut samples = [0.0f32; RING + 200];
        for (i, sample) in samples.iter_mut().enumerate() {
            let t = i as f32 / rate as f32;
            *sample = (t * 330.0 * std::f32::consts::TAU).sin() * 0.75;
        }
        let first = build_tracked(&samples[..RING], rate, &mut mem);
        let second = build_tracked(&samples[200..], rate, &mut mem);
        let peak_a = first.max.iter().position(|s| *s == first.max.iter().copied().max().unwrap());
        let peak_b = second
            .max
            .iter()
            .position(|s| *s == second.max.iter().copied().max().unwrap());
        let shift = peak_a.unwrap().abs_diff(peak_b.unwrap());
        assert!(shift < 8, "picture slid by {shift} columns");
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
}
