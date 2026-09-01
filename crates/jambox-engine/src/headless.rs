//! Null-audio engine loop for hosts and CI with no sound card.
//!
//! Renders real blocks at roughly realtime pace and throws the samples away, so
//! the control socket, sequencer, and MIDI out can be exercised on a dev box (the
//! PLAN's "test logic on the PC without the Pi" loop).
//!
//! The pacing here is wall-clock, so this mode proves *behaviour*, not audio
//! timing. Latency and jitter sign-off still happen on the device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jambox_core::{
    JamboxEngine, LatestTouch, MidiOutSink, ScheduledCommand, WaveBank, MAX_BLOCK_COMMANDS,
    MAX_TOUCH_VOICES,
};
use tracing::info;

use crate::bus::{AudioSide, StatusPacket};

pub fn run(
    sample_rate: u32,
    block: usize,
    mut audio: AudioSide,
    bank: WaveBank,
    running: Arc<AtomicBool>,
) {
    let mut engine = JamboxEngine::with_bank(sample_rate as f64, bank);
    engine.sync_fx_slots();
    let mut midi_out = MidiOutSink::new();
    let mut scheduled: Vec<ScheduledCommand> = Vec::with_capacity(MAX_BLOCK_COMMANDS);
    let mut buf = vec![0.0f32; block];
    let mut touch_scratch = [LatestTouch {
        owner: 0,
        x: 0.0,
        y: 0.0,
        channel: 0,
        velocity: 0,
    }; MAX_TOUCH_VOICES + 3];
    let mut peak_micros = 0u32;
    let mut xruns = 0u64;

    let block_time = Duration::from_secs_f64(block as f64 / sample_rate as f64);
    info!(
        sample_rate,
        block, "headless: running without an audio device"
    );

    let mut next = Instant::now();
    while running.load(Ordering::Relaxed) {
        let started = Instant::now();
        scheduled.clear();

        while let Ok(update) = audio.clips.pop() {
            if let Some(slot) = engine.sequencer_mut().slot_mut(update.slot as usize) {
                if let Some(mode) = update.mode {
                    slot.set_mode(mode);
                }
                if let Some(tone) = update.tone {
                    slot.set_playback_tone(tone);
                } else if update.clip.is_none() {
                    slot.set_playback_tone(1.0);
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
        let n_touch = audio.latest.snapshot(&mut touch_scratch);
        engine.sync_touches(&touch_scratch[..n_touch]);

        engine.render(&mut buf, &scheduled, &mut midi_out);
        for (_frame, event) in midi_out.as_slice() {
            let _ = audio.midi_out.push(*event);
        }
        let micros = started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
        peak_micros = peak_micros.max(micros);
        if started.elapsed() > block_time {
            xruns = xruns.saturating_add(1);
        }
        let _ = audio.status.push(StatusPacket {
            engine: engine.status(),
            callback_frames: block as u32,
            callback_micros: micros,
            callback_peak_micros: peak_micros,
            xruns,
        });

        next += block_time;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}
