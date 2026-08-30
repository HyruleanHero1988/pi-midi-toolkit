//! Audio device wiring: drain rings, render, interleave. Nothing else.
//!
//! The callback body is deliberately boring — every expensive thing (allocating a
//! clip, sending MIDI bytes, writing a log line) happens on another thread.
//!
//! Opening the device is *not* a one-shot. Unplugging a cable (USB audio, or the
//! Pi analog jack with HDMI steal / mixer mute) used to leave a live process
//! writing into a dead or muted ALSA stream. MIDI already hotplugs; audio now
//! does the same: keep the engine, drop the stream, pick a device, unmute, reopen.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, SampleFormat, StreamConfig};
use jambox_core::{
    JamboxEngine, LatestTouch, MidiOutSink, ScheduledCommand, WaveBank, MAX_BLOCK_COMMANDS,
    MAX_RENDER_BLOCK, MAX_TOUCH_VOICES,
};
use tracing::{info, warn};

use crate::bus::{AudioSide, StatusPacket};

/// Conservative first explicit period; architecture primary target is 512.
pub const PREFERRED_BLOCK: u32 = 512;
const SCRATCH_FRAMES: usize = MAX_RENDER_BLOCK;

const HOTPLUG_POLL: Duration = Duration::from_millis(400);
const WATCH_POLL: Duration = Duration::from_millis(200);
const OPEN_GRACE_MS: u64 = 2_000;
const STALE_CALLBACK_MS: u64 = 1_500;
const MIXER_RESTORE_EVERY: Duration = Duration::from_secs(2);
const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(4);

const RANK_FILTER: i32 = 1_000;
const RANK_ANALOG: i32 = 100;

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
    last_callback_ms: AtomicU64,
    error: AtomicBool,
    /// Edge-triggered IPC / operator request to drop and reopen the stream.
    reopen: AtomicBool,
}

impl AudioHealth {
    pub fn new() -> Self {
        Self {
            last_callback_ms: AtomicU64::new(0),
            error: AtomicBool::new(false),
            reopen: AtomicBool::new(false),
        }
    }

    pub fn request_reopen(&self) {
        self.reopen.store(true, Ordering::Relaxed);
    }

    fn take_reopen(&self) -> bool {
        self.reopen.swap(false, Ordering::Relaxed)
    }
}

impl Default for AudioHealth {
    fn default() -> Self {
        Self::new()
    }
}

/// Heap engine + rings. The cpal callback holds a raw pointer at this allocation;
/// the supervisor never moves it while a stream is alive, and never locks it.
struct RenderState {
    audio: AudioSide,
    engine: JamboxEngine,
    scheduled: Vec<ScheduledCommand>,
    midi_out: MidiOutSink,
    mono: Vec<f32>,
    touch_scratch: [LatestTouch; MAX_TOUCH_VOICES + 3],
    peak_micros: u32,
    xruns: u64,
    sample_rate: u32,
    channels: u16,
}

#[derive(Clone, Copy)]
struct StatePtr(*mut RenderState);
// SAFETY: the supervisor owns the Box for the process lifetime and only
// drops the cpal stream (joining the callback) before touching RenderState
// again. The callback never frees it. Send so cpal can move the closure.
unsafe impl Send for StatePtr {}

impl StatePtr {
    fn as_mut(self) -> &'static mut RenderState {
        // SAFETY: see StatePtr Send comment; stream drop joins the callback
        // before the supervisor reuses the Box.
        unsafe { &mut *self.0 }
    }
}

impl RenderState {
    fn new(audio: AudioSide, bank: WaveBank, sample_rate: u32, channels: u16) -> Self {
        let mut engine = JamboxEngine::with_bank(sample_rate as f64, bank);
        engine.sync_fx_slots();
        Self {
            audio,
            engine,
            scheduled: Vec::with_capacity(MAX_BLOCK_COMMANDS),
            midi_out: MidiOutSink::new(),
            mono: vec![0.0; SCRATCH_FRAMES],
            touch_scratch: [LatestTouch {
                owner: 0,
                x: 0.0,
                y: 0.0,
                channel: 0,
                velocity: 0,
            }; MAX_TOUCH_VOICES + 3],
            peak_micros: 0,
            xruns: 0,
            sample_rate,
            channels,
        }
    }

    fn prepare_for_device(&mut self, sample_rate: u32, channels: u16) {
        self.engine.set_sample_rate(sample_rate as f64);
        self.sample_rate = sample_rate;
        self.channels = channels;
    }
}

/// HDMI / vc4 names steal the default after an analog unplug; skip them unless
/// they are the only outputs left.
pub fn is_hdmi_output(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("hdmi") || n.contains("vc4") || n.contains("mai pcm")
}

/// bcm2835 analog / headphone jack (the Pi speaker cable).
pub fn is_analog_output(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("headphone") || n.contains("bcm2835") || n.contains("analog")
}

/// Higher is better. HDMI names are not ranked here — callers skip them first.
pub fn output_rank(name: &str, filter: &str) -> i32 {
    let n = name.to_ascii_lowercase();
    let mut rank = 1;
    if is_analog_output(name) {
        rank += RANK_ANALOG;
    }
    let filter = filter.trim().to_ascii_lowercase();
    if !filter.is_empty() && n.contains(&filter) {
        rank += RANK_FILTER;
    }
    rank
}

/// Choose a device name the same way [`pick_output`] does (pure, for tests).
pub fn select_output_name<'a>(names: &[&'a str], filter: &str) -> Option<&'a str> {
    let mut best: Option<(i32, &'a str)> = None;
    let mut hdmi: Option<&'a str> = None;
    for name in names {
        if is_hdmi_output(name) {
            if hdmi.is_none() {
                hdmi = Some(*name);
            }
            continue;
        }
        let rank = output_rank(name, filter);
        match best {
            None => best = Some((rank, *name)),
            Some((r, _)) if rank > r => best = Some((rank, *name)),
            _ => {}
        }
    }
    best.map(|(_, n)| n).or(hdmi)
}

/// True when the stream should be dropped and the device re-picked.
pub fn should_reopen(
    now_ms: u64,
    opened_at_ms: u64,
    last_callback_ms: u64,
    saw_error: bool,
    grace_ms: u64,
    stale_ms: u64,
) -> bool {
    if saw_error {
        return true;
    }
    if now_ms.saturating_sub(opened_at_ms) < grace_ms {
        return false;
    }
    if last_callback_ms == 0 {
        return true;
    }
    now_ms.saturating_sub(last_callback_ms) >= stale_ms
}

/// Pick an output device, preferring a name substring (e.g. `headphone`).
/// HDMI is skipped while any analog/USB output exists.
pub fn pick_output(name_filter: &str) -> Result<Device, AudioError> {
    let host = cpal::default_host();
    let mut named: Vec<(String, Device)> = Vec::new();
    if let Ok(devices) = host.output_devices() {
        for device in devices {
            let name = device.name().unwrap_or_default();
            named.push((name, device));
        }
    }
    let names: Vec<&str> = named.iter().map(|(n, _)| n.as_str()).collect();
    let Some(chosen) = select_output_name(&names, name_filter) else {
        return host.default_output_device().ok_or(AudioError::NoDevice);
    };
    if is_hdmi_output(chosen) {
        warn!(device = %chosen, "audio: only HDMI outputs found");
    } else if !name_filter.trim().is_empty() && output_rank(chosen, name_filter) < RANK_FILTER {
        warn!(
            filter = %name_filter.trim(),
            picked = %chosen,
            "no output device matched filter; using analog/non-HDMI"
        );
    }
    let chosen = chosen.to_string();
    named
        .into_iter()
        .find(|(n, _)| n == &chosen)
        .map(|(_, d)| d)
        .or_else(|| host.default_output_device())
        .ok_or(AudioError::NoDevice)
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

pub struct RunningStream {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_label: String,
}

/// Own the output for the life of `running`: wait for a card, reopen on death.
pub fn spawn_output(
    filter: String,
    audio: AudioSide,
    bank: WaveBank,
    preferred_frames: u32,
    health: Arc<AudioHealth>,
    running: Arc<AtomicBool>,
) {
    let _ = std::thread::Builder::new()
        .name("jambox-audio-out".into())
        .spawn(move || supervisor(filter, audio, bank, preferred_frames, health, running));
}

fn supervisor(
    filter: String,
    audio: AudioSide,
    bank: WaveBank,
    preferred_frames: u32,
    health: Arc<AudioHealth>,
    running: Arc<AtomicBool>,
) {
    let mut pending_audio = Some(audio);
    let mut pending_bank = Some(bank);
    let mut state: Option<Box<RenderState>> = None;
    let mut backoff = BACKOFF_START;
    let mut announced_wait = false;
    let mut last_mixer = Instant::now()
        .checked_sub(MIXER_RESTORE_EVERY)
        .unwrap_or_else(Instant::now);

    if !filter.trim().is_empty() {
        info!(filter = %filter, "audio: watching output (hotplug)");
    } else {
        info!("audio: watching output (hotplug)");
    }

    while running.load(Ordering::Relaxed) {
        if last_mixer.elapsed() >= MIXER_RESTORE_EVERY {
            restore_mixer();
            last_mixer = Instant::now();
        }

        let device = match pick_output(&filter) {
            Ok(d) => d,
            Err(err) => {
                if !announced_wait {
                    warn!(%err, "audio: no output yet; will grab it when it appears");
                    announced_wait = true;
                }
                std::thread::sleep(HOTPLUG_POLL);
                continue;
            }
        };
        announced_wait = false;
        restore_mixer();
        last_mixer = Instant::now();

        let supported = match device.default_output_config() {
            Ok(c) => c,
            Err(err) => {
                warn!(%err, "audio: device config failed");
                std::thread::sleep(backoff);
                backoff = next_backoff(backoff);
                continue;
            }
        };
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels();

        let render = match state.as_mut() {
            Some(existing) => {
                existing.prepare_for_device(sample_rate, channels);
                existing
            }
            None => {
                state = Some(Box::new(RenderState::new(
                    pending_audio.take().expect("audio side"),
                    pending_bank.take().expect("wave bank"),
                    sample_rate,
                    channels,
                )));
                state.as_mut().expect("state just inserted")
            }
        };

        match open_stream(&device, render, &health, preferred_frames) {
            Ok(stream) => {
                info!(
                    device = %device.name().unwrap_or_default(),
                    sample_rate = stream.sample_rate,
                    channels = stream.channels,
                    buffer = %stream.buffer_label,
                    "audio: running"
                );
                backoff = BACKOFF_START;
                watch_stream(&running, &health, &mut last_mixer);
                drop(stream);
                if running.load(Ordering::Relaxed) {
                    warn!("audio: output died; reopening");
                }
            }
            Err(err) => {
                warn!(%err, "audio: stream failed; retrying");
                std::thread::sleep(backoff);
                backoff = next_backoff(backoff);
            }
        }
    }
}

fn watch_stream(running: &AtomicBool, health: &AudioHealth, last_mixer: &mut Instant) {
    health.error.store(false, Ordering::Relaxed);
    health.last_callback_ms.store(0, Ordering::Relaxed);
    let _ = health.take_reopen();
    let opened_at = now_ms();
    while running.load(Ordering::Relaxed) {
        if last_mixer.elapsed() >= MIXER_RESTORE_EVERY {
            restore_mixer();
            *last_mixer = Instant::now();
        }
        if health.take_reopen()
            || should_reopen(
                now_ms(),
                opened_at,
                health.last_callback_ms.load(Ordering::Relaxed),
                health.error.load(Ordering::Relaxed),
                OPEN_GRACE_MS,
                STALE_CALLBACK_MS,
            )
        {
            break;
        }
        std::thread::sleep(WATCH_POLL);
    }
}

fn next_backoff(prev: Duration) -> Duration {
    prev.saturating_mul(2).min(BACKOFF_MAX)
}

fn now_ms() -> u64 {
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    ORIGIN
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

/// Best-effort analog unmute. Jack-detect / HDMI steal often leaves Headphone
/// muted after a cable pull; this is off the audio thread.
fn restore_mixer() {
    #[cfg(target_os = "linux")]
    {
        use std::process::{Command, Stdio};
        for ctrl in [
            "Headphone",
            "Headphones",
            "PCM",
            "Master",
            "Speaker",
            "Digital",
        ] {
            let _ = Command::new("amixer")
                .args(["-q", "sset", ctrl, "unmute"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Probe which explicit period sizes the device will accept, then open for real.
fn open_stream(
    device: &Device,
    state: &mut RenderState,
    health: &Arc<AudioHealth>,
    preferred_frames: u32,
) -> Result<RunningStream, AudioError> {
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let format = supported.sample_format();
    state.prepare_for_device(sample_rate, channels);

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

    info!(
        device = %device.name().unwrap_or_default(),
        sample_rate,
        channels,
        format = ?format,
        buffer = %chosen,
        "audio: opening output"
    );

    build_stream(device, &config, format, state, Arc::clone(health), chosen)
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

fn build_stream(
    device: &Device,
    config: &StreamConfig,
    format: SampleFormat,
    state: &mut RenderState,
    health: Arc<AudioHealth>,
    chosen: String,
) -> Result<RunningStream, AudioError> {
    let sample_rate = state.sample_rate;
    let channels = state.channels;
    let ptr = StatePtr(state as *mut RenderState);

    let health_err = Arc::clone(&health);
    let err_fn = move |err| {
        health_err.error.store(true, Ordering::Relaxed);
        warn!(%err, "audio stream error");
    };

    let stream = match format {
        SampleFormat::F32 => {
            let health = Arc::clone(&health);
            device.build_output_stream(
                config,
                move |data: &mut [f32], _| {
                    health.last_callback_ms.store(now_ms(), Ordering::Relaxed);
                    callback_body(data, channels as usize, sample_rate, ptr.as_mut(), |s| s);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let health = Arc::clone(&health);
            device.build_output_stream(
                config,
                move |data: &mut [i16], _| {
                    health.last_callback_ms.store(now_ms(), Ordering::Relaxed);
                    callback_body(data, channels as usize, sample_rate, ptr.as_mut(), |s| {
                        (s.clamp(-1.0, 1.0) * 32767.0) as i16
                    });
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let health = Arc::clone(&health);
            device.build_output_stream(
                config,
                move |data: &mut [u16], _| {
                    health.last_callback_ms.store(now_ms(), Ordering::Relaxed);
                    callback_body(data, channels as usize, sample_rate, ptr.as_mut(), |s| {
                        ((s.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65535.0) as u16
                    });
                },
                err_fn,
                None,
            )
        }
        other => {
            return Err(AudioError::Config(format!(
                "unsupported sample format {other:?}"
            )))
        }
    }
    .map_err(|e| AudioError::Build(e.to_string()))?;

    stream
        .play()
        .map_err(|e| AudioError::Build(e.to_string()))?;

    Ok(RunningStream {
        _stream: stream,
        sample_rate,
        channels,
        buffer_label: chosen,
    })
}

/// Render one device block. `to_sample` is the only format-specific work.
fn callback_body<S: Copy>(
    data: &mut [S],
    channels: usize,
    sample_rate: u32,
    state: &mut RenderState,
    to_sample: impl Fn(f32) -> S,
) {
    let started = Instant::now();
    let frames = data.len() / channels.max(1);
    if frames == 0 {
        return;
    }
    if frames > state.mono.len() {
        let zero = to_sample(0.0);
        data.iter_mut().for_each(|s| *s = zero);
        state.xruns = state.xruns.saturating_add(1);
        publish_status(
            &mut state.audio,
            &state.engine,
            frames as u32,
            0,
            state.peak_micros,
            state.xruns,
        );
        return;
    }

    drain(state);
    let n_touch = state.audio.latest.snapshot(&mut state.touch_scratch);
    state.engine.sync_touches(&state.touch_scratch[..n_touch]);

    let mut offset = 0usize;
    while offset < frames {
        let n = (frames - offset).min(MAX_RENDER_BLOCK);
        let cmds: &[ScheduledCommand] = if offset == 0 { &state.scheduled } else { &[] };
        let block = &mut state.mono[offset..offset + n];
        state.engine.render(block, cmds, &mut state.midi_out);
        for (_frame, event) in state.midi_out.as_slice() {
            let _ = state.audio.midi_out.push(*event);
        }
        offset += n;
    }

    for (i, frame_out) in data.chunks_mut(channels).enumerate() {
        let sample = to_sample(block_sample(&state.mono, i));
        for slot in frame_out.iter_mut() {
            *slot = sample;
        }
    }

    let micros = started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
    state.peak_micros = state.peak_micros.max(micros);
    let period_micros = (frames as u64).saturating_mul(1_000_000) / u64::from(sample_rate.max(1));
    if u64::from(micros) > period_micros && period_micros > 0 {
        state.xruns = state.xruns.saturating_add(1);
    }
    publish_status(
        &mut state.audio,
        &state.engine,
        frames as u32,
        micros,
        state.peak_micros,
        state.xruns,
    );
}

fn publish_status(
    audio: &mut AudioSide,
    engine: &JamboxEngine,
    callback_frames: u32,
    callback_micros: u32,
    callback_peak_micros: u32,
    xruns: u64,
) {
    let _ = audio.status.push(StatusPacket {
        engine: engine.status(),
        callback_frames,
        callback_micros,
        callback_peak_micros,
        xruns,
    });
}

fn block_sample(mono: &[f32], i: usize) -> f32 {
    mono.get(i).copied().unwrap_or(0.0)
}

/// Drain both command rings and any clip swaps. Allocation-free.
fn drain(state: &mut RenderState) {
    state.scheduled.clear();

    while let Ok(update) = state.audio.clips.pop() {
        let slot_index = update.slot as usize;
        if let Some(slot) = state.engine.sequencer_mut().slot_mut(slot_index) {
            if let Some(mode) = update.mode {
                slot.set_mode(mode);
            }
            if let Some(previous) = slot.swap_boxed(update.clip) {
                let _ = state.audio.garbage.push(previous);
            }
        }
    }

    // Live MIDI first so a played note is never behind a UI knob move.
    while state.scheduled.len() < MAX_BLOCK_COMMANDS {
        match state.audio.midi_commands.pop() {
            Ok(command) => state.scheduled.push(ScheduledCommand::now(command)),
            Err(_) => break,
        }
    }
    while state.scheduled.len() < MAX_BLOCK_COMMANDS {
        match state.audio.control_commands.pop() {
            Ok(command) => state.scheduled.push(ScheduledCommand::now(command)),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdmi_names_are_detected() {
        assert!(is_hdmi_output("vc4-hdmi-0"));
        assert!(is_hdmi_output("MAI PCM i2s-hifi-0"));
        assert!(is_hdmi_output("bcm2835 HDMI 1"));
        assert!(!is_hdmi_output("bcm2835 Headphones"));
        assert!(!is_hdmi_output("USB Audio Device"));
    }

    #[test]
    fn analog_beats_hdmi_even_when_hdmi_is_defaultish() {
        let names = ["vc4-hdmi", "bcm2835 Headphones"];
        assert_eq!(
            select_output_name(&names, "headphone"),
            Some("bcm2835 Headphones")
        );
        assert_eq!(select_output_name(&names, ""), Some("bcm2835 Headphones"));
    }

    #[test]
    fn analog_preferred_over_usb_when_filter_empty() {
        let names = ["USB Audio Device", "bcm2835 Headphones"];
        assert_eq!(select_output_name(&names, ""), Some("bcm2835 Headphones"));
    }

    #[test]
    fn headphone_filter_picks_headphones_over_generic_usb() {
        let names = ["USB Audio Device", "bcm2835 Headphones", "vc4-hdmi"];
        assert_eq!(
            select_output_name(&names, "headphone"),
            Some("bcm2835 Headphones")
        );
    }

    #[test]
    fn hdmi_is_last_resort() {
        let names = ["vc4-hdmi-0", "vc4-hdmi-1"];
        assert_eq!(select_output_name(&names, "headphone"), Some("vc4-hdmi-0"));
    }

    #[test]
    fn empty_list_is_none() {
        assert_eq!(select_output_name(&[], "headphone"), None);
    }

    #[test]
    fn stream_error_reopens_immediately() {
        assert!(should_reopen(100, 99, 99, true, 2_000, 1_500));
    }

    #[test]
    fn grace_period_waits_for_first_callback() {
        assert!(!should_reopen(500, 0, 0, false, 2_000, 1_500));
        assert!(should_reopen(2_500, 0, 0, false, 2_000, 1_500));
    }

    #[test]
    fn stale_callbacks_reopen() {
        assert!(!should_reopen(3_000, 0, 2_400, false, 2_000, 1_500));
        assert!(should_reopen(4_000, 0, 2_400, false, 2_000, 1_500));
    }

    #[test]
    fn reopen_request_is_edge_triggered() {
        let health = AudioHealth::new();
        assert!(!health.take_reopen());
        health.request_reopen();
        assert!(health.take_reopen());
        assert!(!health.take_reopen());
    }
}
