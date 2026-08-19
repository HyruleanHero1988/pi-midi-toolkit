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

use jambox_core::{JamboxEngine, MidiOutSink, ScheduledCommand, WaveBank, MAX_BLOCK_COMMANDS};
use tracing::info;

use crate::bus::AudioSide;

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

    let block_time = Duration::from_secs_f64(block as f64 / sample_rate as f64);
    info!(
        sample_rate,
        block, "headless: running without an audio device"
    );

    let mut next = Instant::now();
    while running.load(Ordering::Relaxed) {
        scheduled.clear();

        while let Ok(update) = audio.clips.pop() {
            if let Some(slot) = engine.sequencer_mut().slot_mut(update.slot as usize) {
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

        engine.render(&mut buf, &scheduled, &mut midi_out);
        for (_frame, event) in midi_out.as_slice() {
            let _ = audio.midi_out.push(*event);
        }
        let _ = audio.status.push(engine.status());

        next += block_time;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            // Fell behind (debug build, loaded host): resync rather than spiral.
            next = now;
        }
    }
}
