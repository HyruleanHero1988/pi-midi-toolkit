//! Live audio capture → master bus.
//!
//! Output and input are separate cpal streams so a USB mic can feed the Pi
//! headphone jack. The input callback only writes a lock-free mono ring; the
//! output callback drains it and hands samples to [`jambox_core::JamboxEngine`]
//! before bus FX / punch-in.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, SampleFormat, StreamConfig};
use tracing::{info, warn};

use crate::audio::{AudioError, AudioHealth};

const HOTPLUG_POLL: Duration = Duration::from_millis(400);
const WATCH_POLL: Duration = Duration::from_millis(200);
const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(4);

/// ~2 s at 48 kHz — enough to absorb USB jitter without eating RAM.
const RING_CAP: usize = 96_000;

/// Single-producer / single-consumer mono ring. Capacity is power-of-two.
pub struct CaptureRing {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    write: AtomicUsize,
    read: AtomicUsize,
    /// Last observed capture sample rate (0 until the input stream opens).
    sample_rate: AtomicU32,
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
    }

    /// Input callback: push interleaved samples as mono (avg of channels).
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
        let mut space = self.buf.len().saturating_sub(w.wrapping_sub(r).min(self.buf.len()));
        // Leave one slot empty so full vs empty is unambiguous.
        if space > 0 {
            space -= 1;
        }
        let take = frames.min(space);
        for i in 0..take {
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

    /// Output callback: fill `dst` with capture audio resampled to `out_sr`.
    /// Missing samples are silence. Returns how many destination frames got
    /// real capture (rest are zero-filled).
    pub fn pull_resampled(&self, dst: &mut [f32], out_sr: u32) -> usize {
        dst.fill(0.0);
        if dst.is_empty() {
            return 0;
        }
        let in_sr = self.sample_rate.load(Ordering::Relaxed);
        if in_sr == 0 || out_sr == 0 {
            return 0;
        }

        let mut r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        let available = w.wrapping_sub(r);
        if available == 0 {
            return 0;
        }

        // How many input samples this output block wants.
        let want_in = if in_sr == out_sr {
            dst.len()
        } else {
            let n = ((dst.len() as u64 * u64::from(in_sr) + u64::from(out_sr) / 2)
                / u64::from(out_sr)) as usize;
            n.max(1)
        };
        let take = want_in.min(available);

        if in_sr == out_sr {
            let n = take.min(dst.len());
            for i in 0..n {
                // SAFETY: reader owns slot `r`; writer has released it.
                let s = unsafe { *self.buf[r & self.mask].get() };
                dst[i] = s;
                r = r.wrapping_add(1);
            }
            self.read.store(r, Ordering::Release);
            return n;
        }

        // Rate mismatch: advance the read head by `take`, map onto `dst`.
        let start = r;
        r = r.wrapping_add(take);
        self.read.store(r, Ordering::Release);
        let dst_len = dst.len();
        for (i, slot) in dst.iter_mut().enumerate() {
            let src_i = if dst_len <= 1 {
                0
            } else {
                (i as u64 * (take.saturating_sub(1) as u64) / (dst_len as u64 - 1)) as usize
            };
            let idx = start.wrapping_add(src_i);
            *slot = unsafe { *self.buf[idx & self.mask].get() };
        }
        dst_len
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

/// Pick a capture device. Prefers a name substring; skips HDMI when possible.
pub fn pick_input(name_filter: &str) -> Result<Device, AudioError> {
    let host = cpal::default_host();
    let filter = name_filter.trim().to_ascii_lowercase();
    let mut fallback: Option<Device> = None;
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            let name = device.name().unwrap_or_default();
            let lower = name.to_ascii_lowercase();
            if lower.contains("hdmi") || lower.contains("monitor") {
                continue;
            }
            if !filter.is_empty() && lower.contains(&filter) {
                return Ok(device);
            }
            if fallback.is_none() {
                fallback = Some(device);
            }
        }
    }
    fallback
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
    if filter.trim().is_empty() {
        info!("audio: watching for capture (USB mic / line)");
    } else {
        info!(filter = %filter, "audio: watching for capture");
    }

    while running.load(Ordering::Relaxed) {
        let device = match pick_input(&filter) {
            Ok(d) => d,
            Err(_) => {
                if !announced_wait {
                    info!("audio: no capture device yet; punch-in stays synth-only until one appears");
                    announced_wait = true;
                }
                std::thread::sleep(HOTPLUG_POLL);
                continue;
            }
        };
        announced_wait = false;

        match open_input_stream(&device, Arc::clone(&ring), &health) {
            Ok(stream) => {
                info!(
                    device = %device.name().unwrap_or_default(),
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
    let start_gen = health.reopen_gen();
    while running.load(Ordering::Relaxed) {
        if health.reopen_gen() != start_gen {
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
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let format = supported.sample_format();
    let config = StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: BufferSize::Default,
    };
    ring.set_sample_rate(sample_rate);
    ring.clear();

    let health_err = Arc::clone(health);
    let err_fn = move |err| {
        health_err.error.store(true, Ordering::Relaxed);
        warn!(%err, "audio capture error");
    };

    let stream = match format {
        SampleFormat::F32 => {
            let ring = Arc::clone(&ring);
            device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    ring.push_interleaved(data, channels as usize, |s| s);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let ring = Arc::clone(&ring);
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    ring.push_interleaved(data, channels as usize, |s| s as f32 / 32768.0);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let ring = Arc::clone(&ring);
            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    ring.push_interleaved(data, channels as usize, |s| {
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
    .map_err(|e| AudioError::Build(e.to_string()))?;

    stream
        .play()
        .map_err(|e| AudioError::Build(e.to_string()))?;

    Ok(RunningInput {
        _stream: stream,
        sample_rate,
        channels,
    })
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
    fn underrun_is_silence() {
        let ring = CaptureRing::new();
        ring.set_sample_rate(48_000);
        let mut dst = [1.0f32; 16];
        assert_eq!(ring.pull_resampled(&mut dst, 48_000), 0);
        assert!(dst.iter().all(|s| *s == 0.0));
    }
}
