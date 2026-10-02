//! Live audio capture → master bus.
//!
//! Output and input are separate cpal streams so a USB mic can feed the Pi
//! headphone jack. The input callback only writes a lock-free mono ring; the
//! output callback FIFO-drains it (with a small latency cushion) into
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

/// ~250 ms at 48 kHz. Jitter cushion — pull is FIFO with a small target
/// latency; only catch-up-skips when fill exceeds the high-water mark.
const RING_CAP: usize = 12_288;
/// Keep about this much capture queued (smooth, still feels live).
const TARGET_LAT_MS: u32 = 25;
/// Only jump the read pointer when backlog exceeds this.
const HIGH_LAT_MS: u32 = 70;
/// Q0.32 fractional read position between input samples.
const FRAC_ONE: u64 = 1u64 << 32;

/// Single-producer / single-consumer mono ring. Capacity is power-of-two.
pub struct CaptureRing {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    write: AtomicUsize,
    read: AtomicUsize,
    /// Last observed capture sample rate (0 until the input stream opens).
    sample_rate: AtomicU32,
    /// Fractional input position for continuous rate conversion (Q0.32).
    read_frac: AtomicU64,
    /// Last output sample (bit pattern) — held across short underruns.
    hold_bits: AtomicU32,
}

// SAFETY: one writer thread, one reader thread; indices are atomic; slots are
// only written after the reader has advanced past them.
unsafe impl Sync for CaptureRing {}
unsafe impl Send for CaptureRing {}

impl CaptureRing {
    pub fn new() -> Self {
        let cap = RING_CAP.next_power_of_two();
        let mut slots = Vec::with_capacity(cap);
        for _ in 0..cap {
            slots.push(UnsafeCell::new(0.0));
        }
        Self {
            buf: slots.into_boxed_slice(),
            mask: cap - 1,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            sample_rate: AtomicU32::new(0),
            read_frac: AtomicU64::new(0),
            hold_bits: AtomicU32::new(0.0f32.to_bits()),
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
        self.hold_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
    }

    /// Input callback: push interleaved samples as mono (avg of channels).
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
        let max_fill = self.buf.len().saturating_sub(1);
        for i in 0..frames {
            let filled = w.wrapping_sub(r);
            if filled >= max_fill {
                // Drop newest — better a short gap than a torn timeline.
                continue;
            }
            let base = i * channels;
            let mut sum = 0.0f32;
            for c in 0..channels {
                sum += to_f32(data[base + c]);
            }
            let mono = sum / channels as f32;
            // SAFETY: writer owns slot `w`; reader has not claimed it yet.
            unsafe {
                *self.buf[w & self.mask].get() = mono;
            }
            w = w.wrapping_add(1);
        }
        self.write.store(w, Ordering::Release);
    }

    /// Output callback: FIFO drain with linear rate conversion.
    ///
    /// Stays continuous across blocks (fractional phase). Only skips ahead when
    /// fill exceeds [`HIGH_LAT_MS`], and then only down to [`TARGET_LAT_MS`] —
    /// not every block. Short underruns hold the last sample instead of silence.
    pub fn pull_resampled(&self, dst: &mut [f32], out_sr: u32) -> usize {
        if dst.is_empty() {
            return 0;
        }
        let in_sr = self.sample_rate.load(Ordering::Relaxed);
        if in_sr == 0 || out_sr == 0 {
            dst.fill(0.0);
            return 0;
        }
        let w = self.write.load(Ordering::Acquire);
        self.pull_resampled_inner(dst, in_sr, out_sr, w)
    }

    fn pull_resampled_inner(
        &self,
        dst: &mut [f32],
        in_sr: u32,
        out_sr: u32,
        w: usize,
    ) -> usize {
        let mut r = self.read.load(Ordering::Relaxed);
        let mut available = w.wrapping_sub(r);
        let high = ((u64::from(in_sr) * u64::from(HIGH_LAT_MS)) / 1000) as usize;
        let target = ((u64::from(in_sr) * u64::from(TARGET_LAT_MS)) / 1000) as usize;
        let target = target.max(32).min(high.saturating_sub(1).max(32));

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
        let mut hold = f32::from_bits(self.hold_bits.load(Ordering::Relaxed));
        let mut produced = 0usize;
        let mut whole_consumed = 0usize;

        for slot in dst.iter_mut() {
            if whole_consumed >= available {
                *slot = hold;
                continue;
            }
            let frac_f = (frac as f32) * (1.0 / FRAC_ONE as f32);
            let s0 = unsafe { *self.buf[r.wrapping_add(whole_consumed) & self.mask].get() };
            let s1 = if whole_consumed + 1 < available {
                unsafe { *self.buf[r.wrapping_add(whole_consumed + 1) & self.mask].get() }
            } else {
                s0
            };
            let sample = s0 + (s1 - s0) * frac_f;
            *slot = sample;
            hold = sample;
            produced += 1;

            let next = u128::from(frac) + u128::from(step);
            frac = (next & (u128::from(FRAC_ONE) - 1)) as u64;
            whole_consumed += (next >> 32) as usize;
        }

        let advance = whole_consumed.min(available);
        self.read.store(r.wrapping_add(advance), Ordering::Release);
        self.read_frac.store(frac, Ordering::Release);
        self.hold_bits.store(hold.to_bits(), Ordering::Relaxed);
        produced
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
            if lower.contains("hdmi")
                || lower.contains("monitor")
                || lower.contains("pulse")
                || lower.contains("jack")
                || lower.contains("pipewire")
            {
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
                    ring.push_interleaved(data, channels, |s| {
                        (s as f32 / 65535.0) * 2.0 - 1.0
                    });
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

    #[test]
    fn ring_round_trips_mono() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        let src: Vec<f32> = (0..64).map(|i| i as f32 / 64.0).collect();
        ring.push_interleaved(&src, 1, |s| s);
        let mut dst = [0.0f32; 64];
        assert_eq!(ring.pull_resampled(&mut dst, 48_000), 64);
        for i in 0..64 {
            assert!((dst[i] - src[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn stereo_input_averages_to_mono() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(44_100);
        let interleaved = [0.0f32, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
        ring.push_interleaved(&interleaved, 2, |s| s);
        let mut dst = [0.0f32; 4];
        assert_eq!(ring.pull_resampled(&mut dst, 44_100), 4);
        assert!(dst.iter().all(|s| (*s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn underrun_holds_silence_when_empty() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        let mut dst = [1.0f32; 16];
        assert_eq!(ring.pull_resampled(&mut dst, 48_000), 0);
        assert!(dst.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn resample_44k1_from_48k_is_smooth() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        // Rising ramp — nearest-neighbor would stair-step; linear should
        // land between adjacent input samples.
        let src: Vec<f32> = (0..960).map(|i| i as f32 / 960.0).collect();
        ring.push_interleaved(&src, 1, |s| s);
        let mut dst = [0.0f32; 882];
        assert_eq!(ring.pull_resampled(&mut dst, 44_100), 882);
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
        let mut dst = [0.0f32; 64];
        assert_eq!(ring.pull_resampled(&mut dst, 48_000), 64);
        for i in 0..64 {
            assert!(
                (dst[i] - i as f32).abs() < 1e-6,
                "dst[{i}]={} expected FIFO head {}",
                dst[i],
                i
            );
        }
        // Next block continues where the first left off.
        assert_eq!(ring.pull_resampled(&mut dst, 48_000), 64);
        for i in 0..64 {
            assert!((dst[i] - (64 + i) as f32).abs() < 1e-6);
        }
    }

    #[test]
    fn catch_up_only_when_above_high_water() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        // ~100 ms at 48 kHz — above HIGH_LAT_MS (70), so catch-up to ~25 ms.
        let n = 4_800;
        let src: Vec<f32> = (0..n).map(|i| i as f32).collect();
        ring.push_interleaved(&src, 1, |s| s);
        let mut dst = [0.0f32; 64];
        ring.pull_resampled(&mut dst, 48_000);
        // After catch-up, head should be near (n - target), not 0 and not n-64.
        let target = (48_000 * TARGET_LAT_MS / 1000) as usize;
        let expected_head = n - target;
        assert!(
            (dst[0] - expected_head as f32).abs() < 2.0,
            "catch-up should land near target latency, got {} want ~{expected_head}",
            dst[0]
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
        // Full ring is above high-water, so pull catch-ups to target latency —
        // still reading the kept (older) timeline, never the dropped 9999.
        let target = (48_000 * TARGET_LAT_MS / 1000) as usize;
        let mut dst = vec![0.0f32; target.min(512)];
        ring.pull_resampled(&mut dst, 48_000);
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
}
