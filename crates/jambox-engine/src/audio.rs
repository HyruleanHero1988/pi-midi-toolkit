//! Audio device wiring: drain rings, render, interleave. Nothing else.
//!
//! The callback body is deliberately boring — every expensive thing (allocating a
//! clip, sending MIDI bytes, writing a log line) happens on another thread.

use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use jambox_core::{
    JamboxEngine, MidiOutSink, ScheduledCommand, WaveBank, MAX_BLOCK_COMMANDS, MAX_RENDER_BLOCK,
};
use tracing::{info, warn};

use crate::bus::AudioSide;

/// Same ballpark as the Python kiosk (`MIDI_TONE_BLOCKSIZE=1536`).
pub const PREFERRED_BLOCK: u32 = 1536;
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
}

/// Build and start the output stream.
pub fn start(device: &Device, mut audio: AudioSide, bank: WaveBank) -> Result<RunningStream, AudioError> {
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let format = supported.sample_format();
    let (config, chosen) = pick_stream_config(channels, sample_rate);

    info!(
        device = %device.name().unwrap_or_default(),
        sample_rate,
        channels,
        format = ?format,
        buffer = %chosen,
        "audio: opening output"
    );

    let mut engine = JamboxEngine::with_bank(sample_rate as f64, bank);
    engine.sync_fx_slots();
    let mut midi_out = MidiOutSink::new();
    let mut scheduled: Vec<ScheduledCommand> = Vec::with_capacity(MAX_BLOCK_COMMANDS);
    let mut mono: Vec<f32> = vec![0.0; SCRATCH_FRAMES];

    let err_fn = |err| warn!(%err, "audio stream error");

    let stream = match format {
        SampleFormat::F32 => device.build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                callback_body(
                    data,
                    channels as usize,
                    &mut audio,
                    &mut engine,
                    &mut scheduled,
                    &mut midi_out,
                    &mut mono,
                    |s| s,
                );
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_output_stream(
            &config,
            move |data: &mut [i16], _| {
                callback_body(
                    data,
                    channels as usize,
                    &mut audio,
                    &mut engine,
                    &mut scheduled,
                    &mut midi_out,
                    &mut mono,
                    |s| (s.clamp(-1.0, 1.0) * 32767.0) as i16,
                );
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_output_stream(
            &config,
            move |data: &mut [u16], _| {
                callback_body(
                    data,
                    channels as usize,
                    &mut audio,
                    &mut engine,
                    &mut scheduled,
                    &mut midi_out,
                    &mut mono,
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
    })
}

fn pick_stream_config(channels: u16, sample_rate: u32) -> (StreamConfig, String) {
    // Fixed(256) is Invalid argument on bcm2835 Headphones. Default opens and
    // typically yields ~100 ms / ~4410 frames — now fully rendered.
    let config = StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };
    (config, "alsa-default".into())
}

/// Render one device block. `to_sample` is the only format-specific work.
fn callback_body<S: Copy>(
    data: &mut [S],
    channels: usize,
    audio: &mut AudioSide,
    engine: &mut JamboxEngine,
    scheduled: &mut Vec<ScheduledCommand>,
    midi_out: &mut MidiOutSink,
    mono: &mut [f32],
    to_sample: impl Fn(f32) -> S,
) {
    let frames = data.len() / channels.max(1);
    static LOGGED_FRAMES: AtomicBool = AtomicBool::new(false);
    if !LOGGED_FRAMES.swap(true, Ordering::Relaxed) {
        info!(frames, "audio: callback frames");
    }
    if frames == 0 {
        return;
    }
    if frames > mono.len() {
        warn!(frames, scratch = mono.len(), "audio: callback larger than scratch; silencing");
        let zero = to_sample(0.0);
        data.iter_mut().for_each(|s| *s = zero);
        return;
    }

    drain(audio, engine, scheduled);
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

    let _ = audio.status.push(engine.status());
}

fn block_sample(mono: &[f32], i: usize) -> f32 {
    mono.get(i).copied().unwrap_or(0.0)
}

/// Drain both command rings and any clip swaps. Allocation-free.
fn drain(audio: &mut AudioSide, engine: &mut JamboxEngine, scheduled: &mut Vec<ScheduledCommand>) {
    scheduled.clear();

    // Clip swaps: move the pointer in, send the old allocation back to be freed.
    while let Ok(update) = audio.clips.pop() {
        let slot_index = update.slot as usize;
        if let Some(slot) = engine.sequencer_mut().slot_mut(slot_index) {
            let previous = slot.take_clip();
            slot.set_clip(update.clip.map(|b| *b));
            if let Some(mode) = update.mode {
                slot.set_mode(mode);
            }
            if let Some(previous) = previous {
                let _ = audio.garbage.push(Box::new(previous));
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
