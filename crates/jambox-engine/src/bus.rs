//! Lock-free plumbing between the audio thread and everything else.
//!
//! Rules this module exists to enforce:
//!
//! * The audio thread never locks, allocates, frees, or does I/O.
//! * Clips are allocated on the control thread and *freed* there too — the audio
//!   thread only swaps a pointer and hands the old one back through `garbage`.
//! * MIDI out is queued, not written, from the callback; a sender thread does the
//!   syscall.
//! * Continuous XY uses [`LatestMailbox`], not the reliable command ring.

use std::sync::Arc;

use jambox_core::{Clip, Command, EngineStatus, LaunchMode};
use midi_core::MidiEvent;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::mailbox::LatestMailbox;

/// Commands queued between two audio blocks.
const COMMAND_CAPACITY: usize = 1024;
/// Clip swaps in flight.
const CLIP_CAPACITY: usize = 64;
/// Status snapshots (the reader keeps the newest).
const STATUS_CAPACITY: usize = 8;
/// MIDI events waiting for the sender thread.
const MIDI_OUT_CAPACITY: usize = 1024;

/// A clip handed to the audio thread, or `None` to clear the slot.
pub struct ClipUpdate {
    pub slot: u8,
    pub clip: Option<Box<Clip>>,
    pub mode: Option<LaunchMode>,
}

/// Musical snapshot plus the host callback counters (not DSP state).
#[derive(Debug, Clone, Copy, Default)]
pub struct StatusPacket {
    pub engine: EngineStatus,
    pub callback_frames: u32,
    pub callback_micros: u32,
    pub callback_peak_micros: u32,
    pub xruns: u64,
}

/// Producer half held by the control (IPC) thread.
pub struct ControlSide {
    pub commands: Producer<Command>,
    pub clips: Producer<ClipUpdate>,
    pub status: Consumer<StatusPacket>,
    pub garbage: Consumer<Box<Clip>>,
    pub latest: Arc<LatestMailbox>,
}

impl ControlSide {
    /// Queue a command. Returns false when the ring is full (UI is spamming).
    pub fn send(&mut self, command: Command) -> bool {
        self.commands.push(command).is_ok()
    }

    pub fn send_clip(&mut self, update: ClipUpdate) -> bool {
        self.clips.push(update).is_ok()
    }

    /// Newest status the audio thread published, if any.
    pub fn latest_status(&mut self) -> Option<StatusPacket> {
        let mut latest = None;
        while let Ok(status) = self.status.pop() {
            latest = Some(status);
        }
        latest
    }

    /// Drop clips the audio thread replaced. Frees happen here, not in audio.
    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }
}

/// Producer half held by the MIDI input thread.
pub struct MidiInSide {
    pub commands: Producer<Command>,
}

impl MidiInSide {
    pub fn send(&mut self, command: Command) -> bool {
        self.commands.push(command).is_ok()
    }
}

/// Consumer half held by the MIDI output thread.
pub struct MidiOutSide {
    pub events: Consumer<MidiEvent>,
}

/// Everything the audio callback owns.
pub struct AudioSide {
    pub control_commands: Consumer<Command>,
    pub midi_commands: Consumer<Command>,
    pub clips: Consumer<ClipUpdate>,
    pub status: Producer<StatusPacket>,
    pub garbage: Producer<Box<Clip>>,
    pub midi_out: Producer<MidiEvent>,
    pub latest: Arc<LatestMailbox>,
}

/// Build every ring and split it into thread-owned halves.
pub fn channel() -> (ControlSide, MidiInSide, MidiOutSide, AudioSide) {
    let (control_tx, control_rx) = RingBuffer::<Command>::new(COMMAND_CAPACITY);
    let (midi_tx, midi_rx) = RingBuffer::<Command>::new(COMMAND_CAPACITY);
    let (clip_tx, clip_rx) = RingBuffer::<ClipUpdate>::new(CLIP_CAPACITY);
    let (status_tx, status_rx) = RingBuffer::<StatusPacket>::new(STATUS_CAPACITY);
    let (garbage_tx, garbage_rx) = RingBuffer::<Box<Clip>>::new(CLIP_CAPACITY);
    let (midi_out_tx, midi_out_rx) = RingBuffer::<MidiEvent>::new(MIDI_OUT_CAPACITY);
    let latest = Arc::new(LatestMailbox::new());

    (
        ControlSide {
            commands: control_tx,
            clips: clip_tx,
            status: status_rx,
            garbage: garbage_rx,
            latest: Arc::clone(&latest),
        },
        MidiInSide { commands: midi_tx },
        MidiOutSide {
            events: midi_out_rx,
        },
        AudioSide {
            control_commands: control_rx,
            midi_commands: midi_rx,
            clips: clip_rx,
            status: status_tx,
            garbage: garbage_tx,
            midi_out: midi_out_tx,
            latest,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use jambox_core::{ClipEvent, ClipEventKind};

    #[test]
    fn commands_cross_the_ring_in_order() {
        let (mut control, _midi_in, _midi_out, mut audio) = channel();
        assert!(control.send(Command::AllNotesOff));
        assert!(control.send(Command::Panic));
        assert_eq!(audio.control_commands.pop().unwrap(), Command::AllNotesOff);
        assert_eq!(audio.control_commands.pop().unwrap(), Command::Panic);
        assert!(audio.control_commands.pop().is_err());
    }

    #[test]
    fn midi_and_control_use_separate_rings() {
        let (mut control, mut midi_in, _midi_out, mut audio) = channel();
        control.send(Command::Panic);
        midi_in.send(Command::AllNotesOff);
        assert_eq!(audio.control_commands.pop().unwrap(), Command::Panic);
        assert_eq!(audio.midi_commands.pop().unwrap(), Command::AllNotesOff);
    }

    #[test]
    fn a_full_ring_reports_instead_of_blocking() {
        let (mut control, _midi_in, _midi_out, _audio) = channel();
        let mut accepted = 0;
        for _ in 0..(COMMAND_CAPACITY + 64) {
            if control.send(Command::Panic) {
                accepted += 1;
            }
        }
        assert!(accepted <= COMMAND_CAPACITY);
        assert!(!control.send(Command::Panic), "full ring must refuse");
    }

    #[test]
    fn replaced_clips_are_freed_on_the_control_thread() {
        let (mut control, _midi_in, _midi_out, mut audio) = channel();
        let clip = Box::new(Clip::new(
            vec![ClipEvent {
                tick: 0,
                kind: ClipEventKind::NoteOn {
                    channel: 0,
                    note: 60,
                    velocity: 100,
                },
            }],
            960,
        ));
        control.send_clip(ClipUpdate {
            slot: 0,
            clip: Some(clip),
            mode: None,
        });
        let update = audio.clips.pop().unwrap();
        audio.garbage.push(update.clip.unwrap()).unwrap();
        control.collect_garbage();
    }

    #[test]
    fn status_reader_keeps_only_the_newest() {
        let (mut control, _midi_in, _midi_out, mut audio) = channel();
        for position in 0..3u64 {
            audio
                .status
                .push(StatusPacket {
                    engine: EngineStatus {
                        position,
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .unwrap();
        }
        assert_eq!(control.latest_status().unwrap().engine.position, 2);
        assert!(control.latest_status().is_none());
    }
}
