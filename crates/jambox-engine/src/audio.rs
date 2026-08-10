//! Audio device wiring: drain rings, render, interleave. Nothing else.
//!
//! The callback body is deliberately boring — every expensive thing (allocating a
//! clip, sending MIDI bytes, writing a log line) happens on another thread.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use jambox_core::{JamboxEngine, MidiOutSink, ScheduledCommand, MAX_BLOCK_COMMANDS};
use tracing::{info, warn};

use crate::bus::AudioSide;

/// Preferred block size. Small enough to feel immediate, big enough for Pi 2.
pub const PREFERRED_BLOCK: u32 = 256;

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
pub fn start(device: &Device, mut audio: AudioSide) -> Result<RunningStream, AudioError> {
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let config = StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Fixed(PREFERRED_BLOCK),
    };

    info!(
        device = %device.name().unwrap_or_default(),
        sample_rate,
        channels,
        block = PREFERRED_BLOCK,
        "audio: opening output"
    );

    let mut engine = JamboxEngine::new(sample_rate as f64);
    engine.sync_fx_slots();
    let mut midi_out = MidiOutSink::new();
    // Preallocated so the callback never grows a Vec.
    let mut scheduled: Vec<ScheduledCommand> = Vec::with_capacity(MAX_BLOCK_COMMANDS);
    let mut mono: Vec<f32> = vec![0.0; 4096];

    let err_fn = |err| warn!(%err, "audio stream error");

    let stream = match supported.sample_format() {
        SampleFormat::F32 => device.build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                let channels = channels as usize;
                let frames = data.len() / channels.max(1);
                if frames > mono.len() {
                    // Device handed us a bigger block than expected: render what we can
                    // rather than allocating on the audio thread.
                    data.iter_mut().for_each(|s| *s = 0.0);
                    return;
                }

                drain(&mut audio, &mut engine, &mut scheduled);
                let block = &mut mono[..frames];
                engine.render(block, &scheduled, &mut midi_out);

                for (frame, event) in midi_out.as_slice() {
                    let _ = frame;
                    // Full ring means the sender thread is behind; dropping is better
                    // than blocking the audio callback.
                    let _ = audio.midi_out.push(*event);
                }

                for (i, frame_out) in data.chunks_mut(channels).enumerate() {
                    let sample = block[i];
                    for slot in frame_out.iter_mut() {
                        *slot = sample;
                    }
                }

                let _ = audio.status.push(engine.status());
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