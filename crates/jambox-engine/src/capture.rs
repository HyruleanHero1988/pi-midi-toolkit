//! Live audio capture → master bus.
//!
//! Output and input are separate cpal streams so a USB mic can feed the Pi
//! headphone jack. The input callback only writes a lock-free stereo ring; the
//! output callback FIFO-drains it (keeping one output block queued) into
//! [`jambox_core::JamboxEngine`] before punch-in.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, SampleFormat, StreamConfig};
use tracing::{info, warn};

use crate::audio::{AudioError, AudioHealth};

const HOTPLUG_POLL: Duration = Duration::from_millis(400);
const WATCH_POLL: Duration = Duration::from_millis(200);
const BACKOFF_START: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(8);

/// ~340 ms at 48 kHz. Has to hold a full headphone period plus cushion.
const RING_CAP: usize = 16_384;
/// Input kept after the samples this output block will consume.
/// The headphone period is 4096 frames (~93 ms). A cushion shorter than that
/// made every block skip, then hold the tail — a flat stretch on the wave.
const CUSHION_MS: u32 = 80;
/// Extra fill allowed above the cushion before the read pointer jumps ahead.
const CATCHUP_MS: u32 = 80;
/// Q0.32 fractional read position between input samples.
const FRAC_ONE: u64 = 1u64 << 32;

/// Single-producer / single-consumer stereo ring (interleaved L/R). Capacity
/// is power-of-two **frames**.
pub struct CaptureRing {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    write: AtomicUsize,
    read: AtomicUsize,
    /// Last observed capture sample rate (0 until the input stream opens).
    sample_rate: AtomicU32,
    /// Fractional input position for continuous rate conversion (Q0.32).
    read_frac: AtomicU64,
    /// Last output samples (bit patterns) — held across short underruns.
    hold_l: AtomicU32,
    hold_r: AtomicU32,
}

// SAFETY: one writer thread, one reader thread; indices are atomic; slots are
// only written after the reader has advanced past them.
unsafe impl Sync for CaptureRing {}
unsafe impl Send for CaptureRing {}

impl CaptureRing {
    pub fn new() -> Self {
        let cap = RING_CAP.next_power_of_two();
        let mut slots = Vec::with_capacity(cap * 2);
        for _ in 0..cap * 2 {
            slots.push(UnsafeCell::new(0.0));
        }
        Self {
            buf: slots.into_boxed_slice(),
            mask: cap - 1,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            sample_rate: AtomicU32::new(0),
            read_frac: AtomicU64::new(0),
            hold_l: AtomicU32::new(0.0f32.to_bits()),
            hold_r: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    pub fn set_sample_rate(&self, sr: u32) {
        self.sample_rate.store(sr, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    pub fn clear(&self) {
        self.read
            .store(self.write.load(Ordering::Acquire), Ordering::Release);
        self.read_frac.store(0, Ordering::Release);
        self.hold_l.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.hold_r.store(0.0f32.to_bits(), Ordering::Relaxed);
    }

    fn frame_cap(&self) -> usize {
        self.mask + 1
    }

    /// Input callback: push interleaved samples as stereo (L/R).
    /// Mono sources are duplicated. Extra channels beyond 2 are ignored.
    /// If the ring is full, newest frames are dropped so the reader keeps a
    /// continuous stream (producer never races the read index).
    pub fn push_interleaved<S>(&self, data: &[S], channels: usize, to_f32: impl Fn(S) -> f32)
    where
        S: Copy,
    {
        let channels = channels.max(1);
        let frames = data.len() / channels;
        if frames == 0 {
            return;
        }
        let mut w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let max_fill = self.frame_cap().saturating_sub(1);
        for i in 0..frames {
            let filled = w.wrapping_sub(r);
            if filled >= max_fill {
                // Drop newest — better a short gap than a torn timeline.
                continue;
            }
            let base = i * channels;
            let left = to_f32(data[base]);
            let right = if channels > 1 {
                to_f32(data[base + 1])
            } else {
                left
            };
            let slot = (w & self.mask) * 2;
            // SAFETY: writer owns frame `w`; reader has not claimed it yet.
            unsafe {
                *self.buf[slot].get() = left;
                *self.buf[slot + 1].get() = right;
            }
            w = w.wrapping_add(1);
        }
        self.write.store(w, Ordering::Release);
    }

    /// Output callback: FIFO drain with linear rate conversion.
    ///
    /// Stays continuous across blocks (fractional phase). The queue target is
    /// one output block plus [`CUSHION_MS`], so a 48 kHz mic can fill a 44.1 kHz
    /// headphone period without holding the last sample. The read pointer jumps
    /// only when fill is another [`CATCHUP_MS`] above that. A real underrun
    /// still holds the last sample.
    pub fn pull_resampled(&self, left: &mut [f32], right: &mut [f32], out_sr: u32) -> usize {
        let n = left.len().min(right.len());
        if n == 0 {
            return 0;
        }
        let left = &mut left[..n];
        let right = &mut right[..n];
        let in_sr = self.sample_rate.load(Ordering::Relaxed);
        if in_sr == 0 || out_sr == 0 {
            left.fill(0.0);
            right.fill(0.0);
            return 0;
        }
        let w = self.write.load(Ordering::Acquire);
        self.pull_resampled_inner(left, right, in_sr, out_sr, w)
    }

    fn pull_resampled_inner(
        &self,
        left: &mut [f32],
        right: &mut [f32],
        in_sr: u32,
        out_sr: u32,
        w: usize,
    ) -> usize {
        let mut r = self.read.load(Ordering::Relaxed);
        let mut available = w.wrapping_sub(r);
        let (target, high) = Self::queue_marks(in_sr, out_sr, left.len());

        if available > high {
            let skip = available - target;
            r = r.wrapping_add(skip);
            available = target;
            self.read_frac.store(0, Ordering::Relaxed);
        }

        let mut frac = self.read_frac.load(Ordering::Relaxed);
        let step = if in_sr == out_sr {
            FRAC_ONE
        } else {
            ((u128::from(in_sr) << 32) / u128::from(out_sr.max(1))) as u64
        };
        let mut hold_l = f32::from_bits(self.hold_l.load(Ordering::Relaxed));
        let mut hold_r = f32::from_bits(self.hold_r.load(Ordering::Relaxed));
        let mut produced = 0usize;
        let mut whole_consumed = 0usize;

        for (dst_l, dst_r) in left.iter_mut().zip(right.iter_mut()) {
            if whole_consumed >= available {
                *dst_l = hold_l;
                *dst_r = hold_r;
                continue;
            }
            let frac_f = (frac as f32) * (1.0 / FRAC_ONE as f32);
            let i0 = (r.wrapping_add(whole_consumed) & self.mask) * 2;
            let (l0, r0) = unsafe { (*self.buf[i0].get(), *self.buf[i0 + 1].get()) };
            let (l1, r1) = if whole_consumed + 1 < available {
                let i1 = (r.wrapping_add(whole_consumed + 1) & self.mask) * 2;
                unsafe { (*self.buf[i1].get(), *self.buf[i1 + 1].get()) }
            } else {
                (l0, r0)
            };
            let sample_l = l0 + (l1 - l0) * frac_f;
            let sample_r = r0 + (r1 - r0) * frac_f;
            *dst_l = sample_l;
            *dst_r = sample_r;
            hold_l = sample_l;
            hold_r = sample_r;
            produced += 1;

            let next = u128::from(frac) + u128::from(step);
            frac = (next & (u128::from(FRAC_ONE) - 1)) as u64;
            whole_consumed += (next >> 32) as usize;
        }

        let advance = whole_consumed.min(available);
        self.read.store(r.wrapping_add(advance), Ordering::Release);
        self.read_frac.store(frac, Ordering::Release);
        self.hold_l.store(hold_l.to_bits(), Ordering::Relaxed);
        self.hold_r.store(hold_r.to_bits(), Ordering::Relaxed);
        produced
    }

    /// `(target, high)` in input samples. `target` is this output block plus
    /// the cushion. `high` is where a late queue is skipped back to `target`.
    fn queue_marks(in_sr: u32, out_sr: u32, out_frames: usize) -> (usize, usize) {
        let need = ((u64::from(out_frames as u32) * u64::from(in_sr)) / u64::from(out_sr.max(1)))
            as usize
            + 4;
        let cushion = ((u64::from(in_sr) * u64::from(CUSHION_MS)) / 1000) as usize;
        let catchup = ((u64::from(in_sr) * u64::from(CATCHUP_MS)) / 1000) as usize;
        let target = need + cushion.max(32);
        let high = target + catchup.max(32);
        (target, high)
    }

    #[allow(dead_code)]
    pub fn available(&self) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Relaxed);
        w.wrapping_sub(r)
    }
}

impl Default for CaptureRing {
    fn default() -> Self {
        Self::new()
    }
}

/// HDMI / loopback / keyboard composite USB audio — skip unless the filter
/// explicitly names that controller.
pub fn skip_capture_name(name: &str, filter: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.contains("hdmi")
        || lower.contains("monitor")
        || lower.contains("pulse")
        || lower.contains("jack")
        || lower.contains("pipewire")
    {
        return true;
    }
    let filter = filter.trim();
    if midi_core::port_looks_like_controller_io(name) {
        return filter.is_empty() || !midi_core::port_looks_like_controller_io(filter);
    }
    false
}

/// Pick a capture device. Prefers a name substring; skips HDMI / soft plugins
/// when possible; favors USB PnP / `hw:CARD=` mics over Pulse/Jack ghosts.
pub fn pick_input(name_filter: &str) -> Result<Device, AudioError> {
    let host = cpal::default_host();
    let filter = name_filter.trim().to_ascii_lowercase();
    let mut preferred: Option<Device> = None;
    let mut fallback: Option<Device> = None;
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            let name = device.name().unwrap_or_default();
            let lower = name.to_ascii_lowercase();
            if skip_capture_name(&name, &filter) {
                continue;
            }
            if !filter.is_empty() && lower.contains(&filter) {
                return Ok(device);
            }
            let looks_usb = lower.contains("usb")
                || lower.contains("pnp")
                || lower.contains("hw:card=")
                || lower.contains("bluetr");
            if looks_usb && preferred.is_none() {
                preferred = Some(device);
            } else if fallback.is_none() {
                fallback = Some(device);
            }
        }
    }
    preferred
        .or(fallback)
        .or_else(|| host.default_input_device())
        .ok_or(AudioError::NoDevice)
}

pub fn list_inputs() -> Vec<String> {
    let host = cpal::default_host();
    let mut out = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                out.push(name);
            }
        }
    }
    out
}

/// Own the input stream while `running`. Reopens with SET → AUDIO.
pub fn spawn_input(
    filter: String,
    ring: Arc<CaptureRing>,
    health: Arc<AudioHealth>,
    running: Arc<AtomicBool>,
) {
    let _ = std::thread::Builder::new()
        .name("jambox-audio-in".into())
        .spawn(move || input_supervisor(filter, ring, health, running));
}

fn input_supervisor(
    filter: String,
    ring: Arc<CaptureRing>,
    health: Arc<AudioHealth>,
    running: Arc<AtomicBool>,
) {
    let mut backoff = BACKOFF_START;
    let mut announced_wait = false;
    // Held across failed opens. Re-listing inputs opens every ALSA plugin
    // (JACK, Pulse, OSS) and POLLERRs the live headphone stream.
    let mut device: Option<Device> = None;
    let mut last_gen = health.reopen_gen();
    if filter.trim().is_empty() {
        info!("audio: watching for capture (USB mic / line)");
    } else {
        info!(filter = %filter, "audio: watching for capture");
    }

    while running.load(Ordering::Relaxed) {
        let gen = health.reopen_gen();
        if gen != last_gen {
            device = None;
            last_gen = gen;
            backoff = BACKOFF_START;
        }
        if device.is_none() {
            match pick_input(&filter) {
                Ok(d) => {
                    announced_wait = false;
                    device = Some(d);
                }
                Err(_) => {
                    if !announced_wait {
                        info!("audio: no capture device yet; punch-in stays synth-only until one appears");
                        announced_wait = true;
                    }
                    std::thread::sleep(HOTPLUG_POLL);
                    continue;
                }
            }
        }
        let Some(device_ref) = device.as_ref() else {
            continue;
        };

        match open_input_stream(device_ref, Arc::clone(&ring), &health) {
            Ok(stream) => {
                info!(
                    device = %device_ref.name().unwrap_or_default(),
                    sample_rate = stream.sample_rate,
                    channels = stream.channels,
                    "audio: capture running"
                );
                backoff = BACKOFF_START;
                watch_input(&running, &health);
                drop(stream);
                ring.clear();
                ring.set_sample_rate(0);
                if running.load(Ordering::Relaxed) {
                    info!("audio: capture reopen");
                }
            }
            Err(err) => {
                warn!(%err, "audio: capture stream failed; retrying");
                std::thread::sleep(backoff);
                backoff = (backoff.saturating_mul(2)).min(BACKOFF_MAX);
            }
        }
    }
}

fn watch_input(running: &AtomicBool, health: &AudioHealth) {
    health.capture_error.store(false, Ordering::Relaxed);
    let start_gen = health.reopen_gen();
    while running.load(Ordering::Relaxed) {
        if health.reopen_gen() != start_gen {
            break;
        }
        // POLLERR / USB unplug sets this; reopen instead of spinning warn spam.
        if health.capture_error.load(Ordering::Relaxed) {
            warn!("audio: capture error — reopening stream");
            break;
        }
        std::thread::sleep(WATCH_POLL);
    }
}

struct RunningInput {
    _stream: cpal::Stream,
    sample_rate: u32,
    channels: u16,
}

fn open_input_stream(
    device: &Device,
    ring: Arc<CaptureRing>,
    health: &Arc<AudioHealth>,
) -> Result<RunningInput, AudioError> {
    let supported = device
        .default_input_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    // cpal already picks 44.1 kHz when the mic lists it. A second
    // supported-config probe on this USB device returns EPIPE on the open
    // that follows, so use the default config as-is.
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let format = supported.sample_format();
    ring.set_sample_rate(sample_rate);
    ring.clear();

    // USB PnP mics on the Pi wedge hard if we probe Fixed periods that fail
    // (Broken pipe / cannot set freq). Use ALSA Default — the capture ring
    // FIFO-drains with a ~25 ms cushion so monitoring stays continuous.
    let config = StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: BufferSize::Default,
    };
    info!(
        device = %device.name().unwrap_or_default(),
        sample_rate,
        channels,
        format = ?format,
        buffer = "alsa-default",
        "audio: opening capture"
    );
    let stream = build_input_stream(device, &config, format, ring, health)?;

    stream
        .play()
        .map_err(|e| AudioError::Build(e.to_string()))?;

    Ok(RunningInput {
        _stream: stream,
        sample_rate,
        channels,
    })
}

fn build_input_stream(
    device: &Device,
    config: &StreamConfig,
    format: SampleFormat,
    ring: Arc<CaptureRing>,
    health: &Arc<AudioHealth>,
) -> Result<cpal::Stream, AudioError> {
    let channels = config.channels as usize;
    let health_err = Arc::clone(health);
    let last_log_ms = AtomicU64::new(0);
    let err_fn = move |err| {
        health_err.capture_error.store(true, Ordering::Relaxed);
        // ALSA POLLERR can fire every few microseconds; reopen + rate-limit.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let prev = last_log_ms.load(Ordering::Relaxed);
        if now.saturating_sub(prev) >= 2_000 {
            last_log_ms.store(now, Ordering::Relaxed);
            warn!(%err, "audio capture error");
        }
    };

    match format {
        SampleFormat::F32 => {
            let ring = Arc::clone(&ring);
            device.build_input_stream(
                config,
                move |data: &[f32], _| {
                    ring.push_interleaved(data, channels, |s| s);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let ring = Arc::clone(&ring);
            device.build_input_stream(
                config,
                move |data: &[i16], _| {
                    ring.push_interleaved(data, channels, |s| s as f32 / 32768.0);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let ring = Arc::clone(&ring);
            device.build_input_stream(
                config,
                move |data: &[u16], _| {
                    ring.push_interleaved(data, channels, |s| (s as f32 / 65535.0) * 2.0 - 1.0);
                },
                err_fn,
                None,
            )
        }
        other => {
            return Err(AudioError::Config(format!(
                "unsupported capture format {other:?}"
            )))
        }
    }
    .map_err(|e| AudioError::Build(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull(ring: &CaptureRing, n: usize, sr: u32) -> (Vec<f32>, Vec<f32>, usize) {
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        let got = ring.pull_resampled(&mut l, &mut r, sr);
        (l, r, got)
    }

    #[test]
    fn ring_round_trips_mono() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        let src: Vec<f32> = (0..64).map(|i| i as f32 / 64.0).collect();
        ring.push_interleaved(&src, 1, |s| s);
        let (l, r, got) = pull(&ring, 64, 48_000);
        assert_eq!(got, 64);
        for i in 0..64 {
            assert!((l[i] - src[i]).abs() < 1e-6);
            assert!((r[i] - src[i]).abs() < 1e-6, "mono capture duplicates to both sides");
        }
    }

    #[test]
    fn stereo_input_keeps_left_and_right() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(44_100);
        let interleaved = [0.0f32, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        ring.push_interleaved(&interleaved, 2, |s| s);
        let (l, r, got) = pull(&ring, 4, 44_100);
        assert_eq!(got, 4);
        assert!(l.iter().all(|s| s.abs() < 1e-6), "left stayed 0");
        assert!(r.iter().all(|s| (*s - 1.0).abs() < 1e-6), "right stayed 1");
    }

    #[test]
    fn underrun_holds_silence_when_empty() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        let mut l = [1.0f32; 16];
        let mut r = [1.0f32; 16];
        assert_eq!(ring.pull_resampled(&mut l, &mut r, 48_000), 0);
        assert!(l.iter().all(|s| *s == 0.0) && r.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn resample_44k1_from_48k_is_smooth() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        // Rising ramp — nearest-neighbor would stair-step; linear should
        // land between adjacent input samples.
        let src: Vec<f32> = (0..960).map(|i| i as f32 / 960.0).collect();
        ring.push_interleaved(&src, 1, |s| s);
        let (dst, _, got) = pull(&ring, 882, 44_100);
        assert_eq!(got, 882);
        assert!(dst[0] >= 0.0);
        assert!(dst[881] <= 1.0);
        // No huge jumps between adjacent output frames.
        let mut max_step = 0.0f32;
        for w in dst.windows(2) {
            max_step = max_step.max((w[1] - w[0]).abs());
        }
        assert!(
            max_step < 0.05,
            "resample should be smooth, max_step={max_step}"
        );
    }

    #[test]
    fn pull_is_fifo_not_newest_wins() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        let mut src = Vec::with_capacity(1024);
        for i in 0..1024 {
            src.push(i as f32);
        }
        ring.push_interleaved(&src, 1, |s| s);
        let (dst, _, got) = pull(&ring, 64, 48_000);
        assert_eq!(got, 64);
        for i in 0..64 {
            assert!(
                (dst[i] - i as f32).abs() < 1e-6,
                "dst[{i}]={} expected FIFO head {}",
                dst[i],
                i
            );
        }
        // Next block continues where the first left off.
        let (dst, _, got) = pull(&ring, 64, 48_000);
        assert_eq!(got, 64);
        for i in 0..64 {
            assert!((dst[i] - (64 + i) as f32).abs() < 1e-6);
        }
    }

    #[test]
    fn catch_up_only_when_above_high_water() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        let (target, high) = CaptureRing::queue_marks(48_000, 48_000, 64);
        let n = high + 500;
        let src: Vec<f32> = (0..n).map(|i| i as f32).collect();
        ring.push_interleaved(&src, 1, |s| s);
        let (dst, _, _) = pull(&ring, 64, 48_000);
        let expected_head = n - target;
        assert!(
            (dst[0] - expected_head as f32).abs() < 2.0,
            "catch-up should land near target latency, got {} want ~{expected_head}",
            dst[0]
        );
    }

    #[test]
    fn headphone_block_stays_moving_when_a_period_is_queued() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        let out_frames = 4096;
        let (target, _) = CaptureRing::queue_marks(48_000, 44_100, out_frames);
        let src: Vec<f32> = (0..target).map(|i| ((i % 50) as f32) / 49.0).collect();
        ring.push_interleaved(&src, 1, |s| s);
        let (dst, _, got) = pull(&ring, out_frames, 44_100);
        assert_eq!(got, out_frames);
        let mut run = 1usize;
        let mut flat = 1usize;
        for pair in dst.windows(2) {
            if (pair[1] - pair[0]).abs() < 1e-6 {
                run += 1;
                flat = flat.max(run);
            } else {
                run = 1;
            }
        }
        assert!(
            flat < 16,
            "a held tail would draw a flat wave, longest run {flat}"
        );
    }

    #[test]
    fn push_drops_newest_when_full() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        let cap = RING_CAP.next_power_of_two();
        let max_fill = cap - 1;
        let batch: Vec<f32> = (0..max_fill).map(|i| i as f32).collect();
        ring.push_interleaved(&batch, 1, |s| s);
        // Extra sample must not displace older frames (drop newest).
        ring.push_interleaved(&[9_999.0f32], 1, |s| s);
        assert_eq!(ring.available(), max_fill);
        // Full ring is above high-water, so pull catch-ups to the cushion —
        // still reading the kept (older) timeline, never the dropped 9999.
        let (target, _) = CaptureRing::queue_marks(48_000, 48_000, 256);
        let (dst, _, _) = pull(&ring, 256, 48_000);
        let expected_head = max_fill - target;
        assert!(
            (dst[0] - expected_head as f32).abs() < 2.0,
            "kept oldest timeline after catch-up, dst[0]={} want ~{expected_head}",
            dst[0]
        );
        assert!(
            dst.iter().all(|&s| (s - 9_999.0).abs() > 1.0),
            "dropped newest must not appear in output"
        );
    }

    #[test]
    fn smk_usb_audio_is_not_the_default_mic() {
        assert!(skip_capture_name("SMK-37 Elite USB Audio", ""));
        assert!(skip_capture_name("SMK-37 Elite USB Audio", "usb"));
        assert!(!skip_capture_name("SMK-37 Elite USB Audio", "smk"));
        assert!(!skip_capture_name("USB PnP Sound Device", ""));
        assert!(skip_capture_name("pulse", ""));
    }
}
