//! Audio device wiring: drain rings, render, interleave. Nothing else.
//!
//! The callback body is deliberately boring — every expensive thing (allocating a
//! clip, sending MIDI bytes, writing a log line) happens on another thread.

use std::time::Instant;

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

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no output device available")]
    NoDevice,
    #[error("device config error: {0}")]
    Config(String),
    #[error("stream build failed: {0}")]
    Build(String),
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

pub struct RunningStream {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_label: String,
}

/// Probe which explicit period sizes the device will accept, then open for real.
pub fn start(
    device: &Device,
    audio: AudioSide,
    bank: WaveBank,
    preferred_frames: u32,
) -> Result<RunningStream, AudioError> {
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

    info!(
        device = %device.name().unwrap_or_default(),
        sample_rate,
        channels,
        format = ?format,
        buffer = %chosen,
        "audio: opening output"
    );

    build_stream(device, &config, format, audio, bank, sample_rate, channels, chosen)
}

fn probe_config(device: &Device, config: &StreamConfig, format: SampleFormat) -> bool {
    let err_fn = |_err| {};
    let result = match format {
        SampleFormat::F32 => device.build_output_stream(config, |_data: &mut [f32], _| {}, err_fn, None),
        SampleFormat::I16 => device.build_output_stream(config, |_data: &mut [i16], _| {}, err_fn, None),
        SampleFormat::U16 => device.build_output_stream(config, |_data: &mut [u16], _| {}, err_fn, None),
        _ => return false,
    };
    result.is_ok()
}

fn build_stream(
    device: &Device,
    config: &StreamConfig,
    format: SampleFormat,
    mut audio: AudioSide,
    bank: WaveBank,
    sample_rate: u32,
    channels: u16,
    chosen: String,
) -> Result<RunningStream, AudioError> {
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

    let err_fn = |err| warn!(%err, "audio stream error");

    let stream = match format {
        SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _| {
                callback_body(
                    data,
                    channels as usize,
                    sample_rate,
                    &mut audio,
                    &mut engine,
                    &mut scheduled,
                    &mut midi_out,
                    &mut mono,
                    &mut touch_scratch,
                    &mut peak_micros,
                    &mut xruns,
                    |s| s,
                );
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _| {
                callback_body(
                    data,
                    channels as usize,
                    sample_rate,
                    &mut audio,
                    &mut engine,
                    &mut scheduled,
                    &mut midi_out,
                    &mut mono,
                    &mut touch_scratch,
                    &mut peak_micros,
                    &mut xruns,
                    |s| (s.clamp(-1.0, 1.0) * 32767.0) as i16,
                );
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _| {
                callback_body(
                    data,
                    channels as usize,
                    sample_rate,
                    &mut audio,
                    &mut engine,
                    &mut scheduled,
                    &mut midi_out,
                    &mut mono,
                    &mut touch_scratch,
                    &mut peak_micros,
                    &mut xruns,
                    |s| ((s.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65535.0) as u16,
                );
            },
            err_fn,
            None,
        ),
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
    audio: &mut AudioSide,
    engine: &mut JamboxEngine,
    scheduled: &mut Vec<ScheduledCommand>,
    midi_out: &mut MidiOutSink,
    mono: &mut [f32],
    touch_scratch: &mut [LatestTouch],
    peak_micros: &mut u32,
    xruns: &mut u64,
    to_sample: impl Fn(f32) -> S,
) {
    let started = Instant::now();
    let frames = data.len() / channels.max(1);
    if frames == 0 {
        return;
    }
    if frames > mono.len() {
        let zero = to_sample(0.0);
        data.iter_mut().for_each(|s| *s = zero);
        *xruns = xruns.saturating_add(1);
        publish_status(audio, engine, frames as u32, 0, *peak_micros, *xruns);
        return;
    }

    drain(audio, engine, scheduled);
    let n_touch = audio.latest.snapshot(touch_scratch);
    engine.sync_touches(&touch_scratch[..n_touch]);

    let mut offset = 0usize;
    while offset < frames {
        let n = (frames - offset).min(MAX_RENDER_BLOCK);
        let cmds: &[ScheduledCommand] = if offset == 0 { scheduled } else { &[] };
        let block = &mut mono[offset..offset + n];
        engine.render(block, cmds, midi_out);
        for (_frame, event) in midi_out.as_slice() {
            let _ = audio.midi_out.push(*event);
        }
        offset += n;
    }

    for (i, frame_out) in data.chunks_mut(channels).enumerate() {
        let sample = to_sample(block_sample(mono, i));
        for slot in frame_out.iter_mut() {
            *slot = sample;
        }
    }

    let micros = started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
    *peak_micros = (*peak_micros).max(micros);
    let period_micros = (frames as u64).saturating_mul(1_000_000) / u64::from(sample_rate.max(1));
    if u64::from(micros) > period_micros && period_micros > 0 {
        *xruns = xruns.saturating_add(1);
    }
    publish_status(audio, engine, frames as u32, micros, *peak_micros, *xruns);
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
fn drain(audio: &mut AudioSide, engine: &mut JamboxEngine, scheduled: &mut Vec<ScheduledCommand>) {
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

    // Live MIDI first so a played note is never behind a UI knob move.
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
