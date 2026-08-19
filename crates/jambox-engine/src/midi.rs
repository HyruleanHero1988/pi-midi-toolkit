//! MIDI in/out threads.
//!
//! Input: hardware ports are watched and hot-plugged. Bytes (and UI-injected
//! MIDI on the control socket) are parsed once, turned into [`Command`]s, and
//! fanned out to the kiosk. Knob meaning lives here — Tk only displays.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use midi_core::MidiEvent;
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use tracing::{info, warn};

use crate::bus::{MidiInSide, MidiOutSide};
use jambox_core::{Command, FxParam, FxTarget, SynthParam};

/// How often the output thread checks the ring.
const OUT_POLL: Duration = Duration::from_micros(500);
/// How often we look for a newly plugged controller.
const HOTPLUG_POLL: Duration = Duration::from_millis(400);

/// MPK mini factory knobs (Prog Select → Pad 1 / MPC program).
pub const CC_MOD: u8 = 1;
pub const CC_MORPH: u8 = 70;
pub const CC_TONE: u8 = 71;
pub const CC_ATTACK: u8 = 72;
pub const CC_RELEASE: u8 = 73;
pub const CC_VIB_DEPTH: u8 = 74;
pub const CC_VIB_RATE: u8 = 75;
pub const CC_LEVEL: u8 = 77;

pub const MODE_KEYS: u8 = 0;
pub const MODE_DRUMS: u8 = 1;
pub const MODE_FX: u8 = 2;

pub const FX_VOICE: u8 = 0;
pub const FX_DRUM: u8 = 1;
pub const FX_DRUM_GROUP: u8 = 2;
pub const FX_BUS: u8 = 3;

#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    #[error("midi init failed: {0}")]
    Init(String),
    #[error("no MIDI port matched {0:?}")]
    NoPort(String),
    #[error("connect failed: {0}")]
    Connect(String),
}

/// Live knob-bank state. UI buttons write this; MIDI ingest reads it.
#[derive(Default)]
pub struct MidiMap {
    mode: AtomicU8,
    fx_kind: AtomicU8,
    fx_index: AtomicU16,
}

impl MidiMap {
    pub fn set_keys(&self) {
        self.mode.store(MODE_KEYS, Ordering::Relaxed);
    }

    pub fn set_drums(&self) {
        self.mode.store(MODE_DRUMS, Ordering::Relaxed);
    }

    pub fn set_fx(&self, kind: u8, index: u16) {
        self.mode.store(MODE_FX, Ordering::Relaxed);
        self.fx_kind.store(kind, Ordering::Relaxed);
        self.fx_index.store(index, Ordering::Relaxed);
    }

    pub fn apply_knob_map(&self, mode: &str, fx_kind: Option<&str>, fx_index: u16) {
        match mode {
            "drums" => self.set_drums(),
            "fx" => {
                let kind = match fx_kind.unwrap_or("voice") {
                    "drum" => FX_DRUM,
                    "drums" | "drum_group" => FX_DRUM_GROUP,
                    "bus" => FX_BUS,
                    _ => FX_VOICE,
                };
                self.set_fx(kind, fx_index);
            }
            _ => self.set_keys(),
        }
    }

    fn fx_target(&self) -> FxTarget {
        match self.fx_kind.load(Ordering::Relaxed) {
            FX_DRUM => FxTarget::Drum(self.fx_index.load(Ordering::Relaxed) as u8),
            FX_DRUM_GROUP => FxTarget::DrumGroup,
            FX_BUS => FxTarget::Bus,
            _ => FxTarget::Voice(self.fx_index.load(Ordering::Relaxed)),
        }
    }

    /// One MIDI event → zero or more audio-thread commands.
    pub fn interpret(&self, event: MidiEvent) -> Option<Command> {
        match event {
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } => Some(Command::NoteOn {
                channel,
                note,
                velocity,
            }),
            MidiEvent::NoteOff { channel, note, .. } => Some(Command::NoteOff { channel, note }),
            MidiEvent::PitchBend { value, .. } => {
                let semis = ((value as f32) - 8192.0) / 8192.0 * 2.0;
                Some(Command::SetSynth {
                    param: SynthParam::PitchBend,
                    value: semis,
                })
            }
            MidiEvent::ControlChange {
                controller, value, ..
            } => self.interpret_cc(controller, value),
            _ => None,
        }
    }

    fn interpret_cc(&self, controller: u8, value: u8) -> Option<Command> {
        let unit = (value as f32 / 127.0).clamp(0.0, 1.0);
        if controller == CC_MOD {
            return Some(Command::SetSynth {
                param: SynthParam::VibratoDepth,
                value: unit,
            });
        }
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == MODE_FX {
            let param = match controller {
                CC_MORPH => FxParam::Drive,
                CC_TONE => FxParam::DelayTime,
                CC_ATTACK => FxParam::DelayFb,
                CC_RELEASE => FxParam::DelayMix,
                CC_VIB_DEPTH => FxParam::ReverbSize,
                CC_VIB_RATE => FxParam::ReverbMix,
                CC_LEVEL => {
                    return Some(Command::SetSynth {
                        param: SynthParam::Level,
                        value: unit,
                    });
                }
                _ => return None,
            };
            return Some(Command::SetFx {
                target: self.fx_target(),
                param,
                value: unit,
            });
        }
        if mode == MODE_DRUMS {
            let param = match controller {
                CC_MORPH => SynthParam::DrumPitch,
                CC_TONE => SynthParam::DrumTone,
                CC_ATTACK => SynthParam::DrumDecay,
                CC_RELEASE => SynthParam::DrumNoise,
                CC_LEVEL => SynthParam::DrumLevel,
                _ => return None,
            };
            return Some(Command::SetSynth { param, value: unit });
        }
        let param = match controller {
            CC_MORPH => SynthParam::Morph,
            CC_TONE => SynthParam::Tone,
            CC_ATTACK => SynthParam::Attack,
            CC_RELEASE => SynthParam::Release,
            CC_VIB_DEPTH => SynthParam::VibratoDepth,
            CC_VIB_RATE => SynthParam::VibratoRate,
            CC_LEVEL => SynthParam::Level,
            _ => return None,
        };
        Some(Command::SetSynth { param, value: unit })
    }
}

/// Push a MIDI event into DSP and to every UI client.
pub fn ingest(
    event: MidiEvent,
    hub: &crate::ipc::ClientHub,
    map: &MidiMap,
    send: impl FnOnce(Command) -> bool,
) {
    hub.broadcast_midi(event);
    if let Some(command) = map.interpret(event) {
        let _ = send(command);
    }
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

fn is_virtual_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("through") || n.contains("jambox")
}

fn pick_input_name(filter: &str) -> Option<String> {
    let (ins, _) = list_ports();
    let filter_lc = filter.trim().to_ascii_lowercase();
    if !filter_lc.is_empty() {
        return ins
            .into_iter()
            .find(|name| name.to_ascii_lowercase().contains(&filter_lc));
    }
    ins.into_iter().find(|name| !is_virtual_name(name))
}

/// Watch MIDI inputs and connect when a matching port appears (or returns).
pub fn spawn_input(
    filter: String,
    side: Arc<std::sync::Mutex<MidiInSide>>,
    hub: Arc<crate::ipc::ClientHub>,
    map: Arc<MidiMap>,
    running: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let shared_side = side;
        let mut connection: Option<MidiInputConnection<()>> = None;
        let mut current = String::new();
        let mut announced_wait = false;
        if !filter.trim().is_empty() {
            info!(filter = %filter, "midi: watching for input (hotplug)");
        } else {
            info!("midi: watching for a hardware input (hotplug)");
        }
        while running.load(Ordering::Relaxed) {
            let wanted = pick_input_name(&filter);
            let still = wanted.as_ref().map(|n| n == &current).unwrap_or(false)
                && connection.is_some();
            if connection.is_some() && !still {
                info!(port = %current, "midi: input gone; waiting for reconnect");
                connection = None;
                current.clear();
            }
            if connection.is_none() {
                if let Some(name) = wanted {
                    announced_wait = false;
                    match try_connect(&name, Arc::clone(&shared_side), Arc::clone(&hub), Arc::clone(&map))
                    {
                        Ok(conn) => {
                            info!(port = %name, "midi: input open");
                            current = name;
                            connection = Some(conn);
                        }
                        Err(err) => {
                            warn!(%err, port = %name, "midi: input connect failed");
                        }
                    }
                } else if !announced_wait {
                    announced_wait = true;
                    info!("midi: no matching input yet; will grab it when it appears");
                }
            }
            std::thread::sleep(HOTPLUG_POLL);
        }
    });
}

fn try_connect(
    name: &str,
    side: Arc<std::sync::Mutex<MidiInSide>>,
    hub: Arc<crate::ipc::ClientHub>,
    map: Arc<MidiMap>,
) -> Result<MidiInputConnection<()>, MidiError> {
    let mut input = MidiInput::new("jambox-in").map_err(|e| MidiError::Init(e.to_string()))?;
    input.ignore(midir::Ignore::SysexAndTime);
    let ports = input.ports();
    let chosen = ports
        .iter()
        .find(|p| input.port_name(p).ok().as_deref() == Some(name))
        .cloned()
        .ok_or_else(|| MidiError::NoPort(name.to_string()))?;
    input
        .connect(
            &chosen,
            "jambox-in",
            move |_stamp, bytes, _| {
                if let Some(event) = MidiEvent::parse(bytes) {
                    let mut guard = side.lock().unwrap_or_else(|p| p.into_inner());
                    ingest(event, &hub, &map, |c| guard.send(c));
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
            if is_virtual_name(&name) && !filter_lc.is_empty() && !name.contains(&filter_lc) {
                return false;
            }
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
        let map = MidiMap::default();
        let command = map.interpret(MidiEvent::NoteOn {
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
        let map = MidiMap::default();
        let command = map.interpret(MidiEvent::NoteOff {
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
    fn key_knobs_move_synth_params() {
        let map = MidiMap::default();
        let command = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_TONE,
                value: 127,
            })
            .unwrap();
        assert_eq!(
            command,
            Command::SetSynth {
                param: SynthParam::Tone,
                value: 1.0
            }
        );
    }

    #[test]
    fn drum_mode_rebinds_the_same_ccs() {
        let map = MidiMap::default();
        map.set_drums();
        let command = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MORPH,
                value: 64,
            })
            .unwrap();
        match command {
            Command::SetSynth {
                param: SynthParam::DrumPitch,
                value,
            } => assert!((value - 64.0 / 127.0).abs() < 0.001),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn skips_through_and_engine_loopback_names() {
        assert!(is_virtual_name("Midi Through:Midi Through Port-0 14:0"));
        assert!(is_virtual_name("jambox-out:jambox-out 129:0"));
        assert!(!is_virtual_name("MPK mini 3"));
    }
}
