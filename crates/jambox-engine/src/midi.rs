//! MIDI in/out threads.
//!
//! Input: `midir` hands us bytes on its own thread; we parse and push a `Command`.
//! Output: a sender thread drains the ring, so the audio callback never makes a
//! syscall. That costs up to one poll interval of jitter on emitted MIDI and keeps
//! the audio thread clean — the trade the architecture rule asks for.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use midi_core::MidiEvent;
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use tracing::{info, warn};

use crate::bus::{MidiInSide, MidiOutSide};
use jambox_core::Command;

/// How often the output thread checks the ring.
const OUT_POLL: Duration = Duration::from_micros(500);

#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    #[error("midi init failed: {0}")]
    Init(String),
    #[error("no MIDI port matched {0:?}")]
    NoPort(String),
    #[error("connect failed: {0}")]
    Connect(String),
}

/// Translate a wire message into an engine command.
pub fn command_for(event: MidiEvent) -> Option<Command> {
    Some(match event {
        MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        } => Command::NoteOn {
            channel,
            note,
            velocity,
        },
        MidiEvent::NoteOff { channel, note, .. } => Command::NoteOff { channel, note },
        // CC / bend mapping lives in the UI: it knows which knob is bound to what.
        _ => return None,
    })
}

pub fn list_ports() -> (Vec<String>, Vec<String>) {
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    if let Ok(input) = MidiInput::new("jambox-list") {
        for port in input.ports() {
            if let Ok(name) = input.port_name(&port) {
                ins.push(name);
            }
        }
    }
    if let Ok(output) = MidiOutput::new("jambox-list") {
        for port in output.ports() {
            if let Ok(name) = output.port_name(&port) {
                outs.push(name);
            }
        }
    }
    (ins, outs)
}

/// Open a MIDI input whose name contains `filter` (empty = first port).
pub fn open_input(
    filter: &str,
    mut side: MidiInSide,
) -> Result<MidiInputConnection<()>, MidiError> {
    let mut input = MidiInput::new("jambox-in").map_err(|e| MidiError::Init(e.to_string()))?;
    input.ignore(midir::Ignore::SysexAndTime);
    let filter_lc = filter.trim().to_ascii_lowercase();

    let ports = input.ports();
    let chosen = ports
        .iter()
        .find(|p| {
            let name = input.port_name(p).unwrap_or_default().to_ascii_lowercase();
            filter_lc.is_empty() || name.contains(&filter_lc)
        })
        .cloned()
        .ok_or_else(|| MidiError::NoPort(filter.to_string()))?;

    let name = input.port_name(&chosen).unwrap_or_default();
    info!(port = %name, "midi: input open");

    input
        .connect(
            &chosen,
            "jambox-in",
            move |_stamp, bytes, _| {
                if let Some(event) = MidiEvent::parse(bytes) {
                    if let Some(command) = command_for(event) {
                        // Dropping beats blocking: the ring is sized for real playing.
                        let _ = side.send(command);
                    }
                }
            },
            (),
        )
        .map_err(|e| MidiError::Connect(e.to_string()))
}

fn open_output(filter: &str) -> Result<MidiOutputConnection, MidiError> {
    let output = MidiOutput::new("jambox-out").map_err(|e| MidiError::Init(e.to_string()))?;
    let filter_lc = filter.trim().to_ascii_lowercase();
    let ports = output.ports();
    let chosen = ports
        .iter()
        .find(|p| {
            let name = output.port_name(p).unwrap_or_default().to_ascii_lowercase();
            filter_lc.is_empty() || name.contains(&filter_lc)
        })
        .cloned()
        .ok_or_else(|| MidiError::NoPort(filter.to_string()))?;
    let name = output.port_name(&chosen).unwrap_or_default();
    info!(port = %name, "midi: output open");
    output
        .connect(&chosen, "jambox-out")
        .map_err(|e| MidiError::Connect(e.to_string()))
}

/// Drain engine-emitted MIDI to a hardware port until `running` clears.
pub fn spawn_output(filter: String, mut side: MidiOutSide, running: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut connection = match open_output(&filter) {
            Ok(c) => c,
            Err(err) => {
                warn!(%err, "midi: output unavailable; clip MIDI stays local");
                // Still drain so the ring cannot back up and stall the audio thread.
                while running.load(Ordering::Relaxed) {
                    while side.events.pop().is_ok() {}
                    std::thread::sleep(OUT_POLL);
                }
                return;
            }
        };
        let mut buf = [0u8; 3];
        while running.load(Ordering::Relaxed) {
            let mut idle = true;
            while let Ok(event) = side.events.pop() {
                idle = false;
                let n = event.encode(&mut buf);
                let _ = connection.send(&buf[..n]);
            }
            if idle {
                std::thread::sleep(OUT_POLL);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_becomes_a_note_command() {
        let command = command_for(MidiEvent::NoteOn {
            channel: 9,
            note: 36,
            velocity: 110,
        });
        assert_eq!(
            command,
            Some(Command::NoteOn {
                channel: 9,
                note: 36,
                velocity: 110
            })
        );
    }

    #[test]
    fn note_off_becomes_a_release() {
        let command = command_for(MidiEvent::NoteOff {
            channel: 0,
            note: 60,
            velocity: 0,
        });
        assert_eq!(
            command,
            Some(Command::NoteOff {
                channel: 0,
                note: 60
            })
        );
    }

    #[test]
    fn cc_is_left_to_the_ui_to_interpret() {
        assert!(command_for(MidiEvent::ControlChange {
            channel: 0,
            controller: 70,
            value: 64
        })
        .is_none());
    }
}
