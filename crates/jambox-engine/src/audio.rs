//! Audio device wiring: render on a dedicated thread, cpal only plays.
//!
//! The bcm2835 Headphones path often stays "open" but silent after a jack
//! unplug/replug. Keeping the engine off the cpal callback lets us drop and
//! reopen the ALSA stream without wiping voices, clips, or FX state.
//!
//! Rules:
//! * The render path never locks, allocates, frees, or does I/O beyond the
//!   existing ring drains (same bus contract as before).
//! * The device callback only copies interleaved samples from a lock-free ring.
//! * Stream errors, callback stalls, and an IPC reopen request all rebuild PCM.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, SampleFormat, Stream, StreamConfig};
use jambox_core::{
    JamboxEngine, LatestTouch, MidiOutSink, ScheduledCommand, WaveBank, MAX_BLOCK_COMMANDS,
    MAX_RENDER_BLOCK, MAX_TOUCH_VOICES,
};
use rtrb::{Consumer, Producer, RingBuffer};
use tracing::{info, warn};

use crate::bus::{AudioSide, StatusPacket};

/// Conservative first explicit period; architecture primary target is 512.
pub const PREFERRED_BLOCK: u32 = 512;
const SCRATCH_FRAMES: usize = MAX_RENDER_BLOCK;
/// Interleaved float samples buffered ahead of the device (~170 ms @ 48 kHz stereo).
const FRAME_RING_SAMPLES: usize = 16_384;
const RENDER_BLOCK: usize = 256;
const REOPEN_SETTLE: Duration = Duration::from_millis(80);
const STALL_REOPEN_AFTER: Duration = Duration::from_millis(500);
const SUPERVISOR_POLL: Duration = Duration::from_millis(50);
/// After a jack swap the stream often keeps callbacking while silent. Soft-reopen
/// once we've been fully idle so the next note hits a fresh PCM.
const IDLE_AUTO_REOPEN_AFTER: Duration = Duration::from_millis(2_500);

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no output device available")]
    NoDevice,
    #[error("device config error: {0}")]
    Config(String),
    #[error("stream build failed: {0}")]
    Build(String),
}

/// Shared reopen / liveness flags between IPC, the device callback, and the supervisor.
pub struct AudioHealth {
    reopen: AtomicBool,
    /// Milliseconds since an arbitrary epoch; updated every device callback.
    last_callback_ms: AtomicU64,
    stream_playing: AtomicBool,
}

impl AudioHealth {
    pub fn new() -> Self {
        Self {
            reopen: AtomicBool::new(false),
            last_callback_ms: AtomicU64::new(0),
            stream_playing: AtomicBool::new(false),
        }
    }

    pub fn request_reopen(&self) {
        self.reopen.store(true, Ordering::Relaxed);
    }

    pub(crate) fn take_reopen(&self) -> bool {
        self.reopen.swap(false, Ordering::Relaxed)
    }

    fn mark_callback(&self) {
        self.last_callback_ms.store(now_ms(), Ordering::Relaxed);
    }

    fn set_playing(&self, playing: bool) {
        self.stream_playing.store(playing, Ordering::Relaxed);
        if playing {
            self.mark_callback();
        }
    }

    fn stalled(&self, after: Duration) -> bool {
        if !self.stream_playing.load(Ordering::Relaxed) {
            return false;
        }
        let last = self.last_callback_ms.load(Ordering::Relaxed);
        if last == 0 {
            return false;
        }
        now_ms().saturating_sub(last) >= after.as_millis() as u64
    }
}

impl Default for AudioHealth {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_millis() as u64
}

/// Pick an output device, preferring a name substring (e.g. `headphone`).
pub fn pick_output(name_filter: &str) -> Result<Device, AudioError> {
    let host = cpal::default_host();
    let filter = name_filter.trim().to_ascii_lowercase();
    if !filter.is_empty() {
        if let Ok(devices) = host.output_devices() {
            for device in devices {
                let name = device.name().unwrap_or_default().to_ascii_lowercase();
                if name.contains(&filter) {
                    return Ok(device);
                }
            }
        }
        warn!(filter = %filter, "no output device matched; using default");
    }
    host.default_output_device().ok_or(AudioError::NoDevice)
}

/// List output devices for commissioning.
pub fn list_outputs() -> Vec<String> {
    let host = cpal::default_host();
    let mut out = Vec::new();
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                out.push(name);
            }
        }
    }
    out
}

struct DeviceConfig {
    sample_rate: u32,
    channels: u16,
    format: SampleFormat,
    config: StreamConfig,
    buffer_label: String,
}

fn resolve_config(device: &Device, preferred_frames: u32) -> Result<DeviceConfig, AudioError> {
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let format = supported.sample_format();

    // cpal Fixed sizes often probe OK on bcm2835 Headphones then immediately
    // XRUN/POLLERR at runtime. Only try Fixed when the operator asks
    // (--buffer-frames N with N>0); otherwise keep ALSA default periods.
    let mut chosen = "alsa-default".to_string();
    let mut config = StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: BufferSize::Default,
    };

    if preferred_frames > 0 {
        let mut candidates = vec![preferred_frames];
        for size in [PREFERRED_BLOCK, 1024, 256] {
            if !candidates.contains(&size) {
                candidates.push(size);
            }
        }
        for frames in candidates {
            let trial = StreamConfig {
                channels,
                sample_rate: cpal::SampleRate(sample_rate),
                buffer_size: BufferSize::Fixed(frames),
            };
            if probe_config(device, &trial, format) {
                config = trial;
                chosen = format!("fixed-{frames}");
                break;
            }
            warn!(frames, "audio: fixed period rejected");
        }
    }

    Ok(DeviceConfig {
        sample_rate,
        channels,
        format,
        config,
        buffer_label: chosen,
    })
}

fn probe_config(device: &Device, config: &StreamConfig, format: SampleFormat) -> bool {
    let err_fn = |_err| {};
    let result = match format {
        SampleFormat::F32 => {
            device.build_output_stream(config, |_data: &mut [f32], _| {}, err_fn, None)
        }
        SampleFormat::I16 => {
            device.build_output_stream(config, |_data: &mut [i16], _| {}, err_fn, None)
        }
        SampleFormat::U16 => {
            device.build_output_stream(config, |_data: &mut [u16], _| {}, err_fn, None)
        }
        _ => return false,
    };
    result.is_ok()
}

/// Run forever: one render thread owns the engine; the supervisor reopens PCM.
pub fn spawn_supervised(
    output_filter: String,
    preferred_frames: u32,
    audio: AudioSide,
    bank: WaveBank,
    health: Arc<AudioHealth>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        run_supervised(
            output_filter,
            preferred_frames,
            audio,
            bank,
            health,
            running,
        );
    })
}

fn run_supervised(
    output_filter: String,
    preferred_frames: u32,
    audio: AudioSide,
    bank: WaveBank,
    health: Arc<AudioHealth>,
    running: Arc<AtomicBool>,
) {
    let device = match pick_output(&output_filter) {
        Ok(d) => d,
        Err(err) => {
            warn!(%err, "audio: no usable output");
            return;
        }
    };
    let cfg = match resolve_config(&device, preferred_frames) {
        Ok(c) => c,
        Err(err) => {
            warn!(%err, "audio: config failed");
            return;
        }
    };

    info!(
        device = %device.name().unwrap_or_default(),
        sample_rate = cfg.sample_rate,
        channels = cfg.channels,
        format = ?cfg.format,
        buffer = %cfg.buffer_label,
        "audio: opening supervised output"
    );

    let (producer_tx, producer_rx) = mpsc::channel::<Producer<f32>>();
    let (frames_tx, frames_rx) = RingBuffer::<f32>::new(FRAME_RING_SAMPLES);
    let _ = producer_tx.send(frames_tx);

    let render_running = Arc::clone(&running);
    let render_health = Arc::clone(&health);
    let render = thread::spawn(move || {
        render_loop(
            audio,
            bank,
            cfg.sample_rate,
            cfg.channels as usize,
            producer_rx,
            render_health,
            render_running,
        );
    });

    let mut generation = 0u32;
    let mut frames_rx = Some(frames_rx);

    while running.load(Ordering::Relaxed) {
        generation = generation.wrapping_add(1);

        let rx = match frames_rx.take() {
            Some(rx) => rx,
            None => {
                let (tx, rx) = RingBuffer::<f32>::new(FRAME_RING_SAMPLES);
                if producer_tx.send(tx).is_err() {
                    break;
                }
                rx
            }
        };

        let stream = match open_playback_stream(&device, &cfg, rx, Arc::clone(&health)) {
            Ok(s) => s,
            Err(err) => {
                warn!(%err, generation, "audio: stream open failed; retrying");
                // Consumer was consumed by the failed build attempt's drop path, or
                // never installed — always install a fresh ring on the next pass.
                frames_rx = None;
                thread::sleep(REOPEN_SETTLE);
                continue;
            }
        };

        health.set_playing(true);
        info!(generation, buffer = %cfg.buffer_label, "audio: stream playing");

        while running.load(Ordering::Relaxed) {
            if health.take_reopen() {
                info!(generation, "audio: reopen requested");
                break;
            }
            if health.stalled(STALL_REOPEN_AFTER) {
                warn!(generation, "audio: callback stalled; reopening");
                let _ = health.take_reopen();
                break;
            }
            thread::sleep(SUPERVISOR_POLL);
        }

        health.set_playing(false);
        drop(stream);
        // Consumer died with the stream callback; next loop installs a fresh ring.
        frames_rx = None;
        // Give ALSA a moment before reclaiming hw:Headphones after unplug/replug.
        thread::sleep(REOPEN_SETTLE);
    }

    drop(producer_tx);
    let _ = render.join();
}

fn open_playback_stream(
    device: &Device,
    cfg: &DeviceConfig,
    frames_rx: Consumer<f32>,
    health: Arc<AudioHealth>,
) -> Result<Stream, AudioError> {
    let channels = cfg.channels as usize;
    let make_err = |health: Arc<AudioHealth>| {
        move |err| {
            warn!(%err, "audio stream error");
            health.request_reopen();
        }
    };

    let stream = match cfg.format {
        SampleFormat::F32 => {
            let mut frames_rx = frames_rx;
            let health_cb = Arc::clone(&health);
            device.build_output_stream(
                &cfg.config,
                move |data: &mut [f32], _| {
                    fill_output(data, channels, &mut frames_rx, &health_cb, |s| s);
                },
                make_err(Arc::clone(&health)),
                None,
            )
        }
        SampleFormat::I16 => {
            let mut frames_rx = frames_rx;
            let health_cb = Arc::clone(&health);
            device.build_output_stream(
                &cfg.config,
                move |data: &mut [i16], _| {
                    fill_output(data, channels, &mut frames_rx, &health_cb, |s| {
                        (s.clamp(-1.0, 1.0) * 32767.0) as i16
                    });
                },
                make_err(Arc::clone(&health)),
                None,
            )
        }
        SampleFormat::U16 => {
            let mut frames_rx = frames_rx;
            let health_cb = Arc::clone(&health);
            device.build_output_stream(
                &cfg.config,
                move |data: &mut [u16], _| {
                    fill_output(data, channels, &mut frames_rx, &health_cb, |s| {
                        ((s.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65535.0) as u16
                    });
                },
                make_err(Arc::clone(&health)),
                None,
            )
        }
        other => {
            return Err(AudioError::Config(format!(
                "unsupported sample format {other:?}"
            )));
        }
    }
    .map_err(|e| AudioError::Build(e.to_string()))?;

    stream
        .play()
        .map_err(|e| AudioError::Build(e.to_string()))?;
    Ok(stream)
}

fn fill_output<S: Copy>(
    data: &mut [S],
    channels: usize,
    frames_rx: &mut Consumer<f32>,
    health: &AudioHealth,
    to_sample: impl Fn(f32) -> S,
) {
    health.mark_callback();
    let channels = channels.max(1);
    let zero = to_sample(0.0);
    for frame in data.chunks_mut(channels) {
        let sample = match frames_rx.pop() {
            Ok(s) => to_sample(s),
            Err(_) => zero,
        };
        // Ring stores already-interleaved samples (L,R,L,R…). First pop is ch0.
        frame[0] = sample;
        for ch in 1..channels {
            frame[ch] = match frames_rx.pop() {
                Ok(s) => to_sample(s),
                Err(_) => zero,
            };
        }
    }
}

fn render_loop(
    mut audio: AudioSide,
    bank: WaveBank,
    sample_rate: u32,
    channels: usize,
    producer_rx: Receiver<Producer<f32>>,
    health: Arc<AudioHealth>,
    running: Arc<AtomicBool>,
) {
    let mut engine = JamboxEngine::with_bank(sample_rate as f64, bank);
    engine.sync_fx_slots();
    let mut midi_out = MidiOutSink::new();
    let mut scheduled: Vec<ScheduledCommand> = Vec::with_capacity(MAX_BLOCK_COMMANDS);
    let mut mono: Vec<f32> = vec![0.0; SCRATCH_FRAMES];
    let mut touch_scratch = [LatestTouch {
        owner: 0,
        x: 0.0,
        y: 0.0,
        channel: 0,
        velocity: 0,
    }; MAX_TOUCH_VOICES + 3];
    let mut peak_micros = 0u32;
    let mut xruns = 0u64;
    let channels = channels.max(1);
    let idle_reopen_frames =
        ((IDLE_AUTO_REOPEN_AFTER.as_secs_f64() * f64::from(sample_rate)) as u64).max(1);
    let mut idle_frames = 0u64;
    let mut auto_reopened_this_idle = false;

    let mut frames_tx = match producer_rx.recv() {
        Ok(tx) => tx,
        Err(_) => return,
    };

    while running.load(Ordering::Relaxed) {
        while let Ok(next) = producer_rx.try_recv() {
            frames_tx = next;
        }

        let need = RENDER_BLOCK * channels;
        if frames_tx.slots() < need {
            thread::sleep(Duration::from_micros(500));
            continue;
        }

        let started = Instant::now();
        drain_commands(&mut audio, &mut engine, &mut scheduled);
        let n_touch = audio.latest.snapshot(&mut touch_scratch);
        engine.sync_touches(&touch_scratch[..n_touch]);

        let block = &mut mono[..RENDER_BLOCK];
        engine.render(block, &scheduled, &mut midi_out);
        for (_frame, event) in midi_out.as_slice() {
            let _ = audio.midi_out.push(*event);
        }

        for sample in block.iter().copied() {
            for _ in 0..channels {
                let _ = frames_tx.push(sample);
            }
        }

        let status = engine.status();
        if status.active_voices == 0 && status.playing_clips == 0 {
            idle_frames = idle_frames.saturating_add(RENDER_BLOCK as u64);
            if idle_frames >= idle_reopen_frames && !auto_reopened_this_idle {
                health.request_reopen();
                auto_reopened_this_idle = true;
            }
        } else {
            idle_frames = 0;
            auto_reopened_this_idle = false;
        }

        let micros = started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
        peak_micros = peak_micros.max(micros);
        let period_micros =
            (RENDER_BLOCK as u64).saturating_mul(1_000_000) / u64::from(sample_rate.max(1));
        if u64::from(micros) > period_micros && period_micros > 0 {
            xruns = xruns.saturating_add(1);
        }
        let _ = audio.status.push(StatusPacket {
            engine: status,
            callback_frames: RENDER_BLOCK as u32,
            callback_micros: micros,
            callback_peak_micros: peak_micros,
            xruns,
        });
    }
}

fn drain_commands(
    audio: &mut AudioSide,
    engine: &mut JamboxEngine,
    scheduled: &mut Vec<ScheduledCommand>,
) {
    scheduled.clear();

    while let Ok(update) = audio.clips.pop() {
        let slot_index = update.slot as usize;
        if let Some(slot) = engine.sequencer_mut().slot_mut(slot_index) {
            if let Some(mode) = update.mode {
                slot.set_mode(mode);
            }
            if let Some(previous) = slot.swap_boxed(update.clip) {
                let _ = audio.garbage.push(previous);
            }
        }
    }

    while scheduled.len() < MAX_BLOCK_COMMANDS {
        match audio.midi_commands.pop() {
            Ok(command) => scheduled.push(ScheduledCommand::now(command)),
            Err(_) => break,
        }
    }
    while scheduled.len() < MAX_BLOCK_COMMANDS {
        match audio.control_commands.pop() {
            Ok(command) => scheduled.push(ScheduledCommand::now(command)),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopen_request_is_edge_triggered() {
        let health = AudioHealth::new();
        assert!(!health.take_reopen());
        health.request_reopen();
        assert!(health.take_reopen());
        assert!(!health.take_reopen());
    }

    #[test]
    fn stall_detects_missing_callbacks_while_playing() {
        let health = AudioHealth::new();
        health.stream_playing.store(true, Ordering::Relaxed);
        // Wait until the monotonic clock has enough range for a 500 ms age.
        while now_ms() < 600 {
            std::thread::sleep(Duration::from_millis(25));
        }
        health
            .last_callback_ms
            .store(now_ms().saturating_sub(600), Ordering::Relaxed);
        assert!(health.stalled(Duration::from_millis(500)));
        health.mark_callback();
        assert!(!health.stalled(Duration::from_millis(500)));
    }
}
