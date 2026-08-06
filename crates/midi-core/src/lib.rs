//! Pure MIDI transform logic — no device I/O.
//!
//! Designed for a tight hot path: processors take an event and return zero or more
//! output events without allocating when possible (small fixed buffers / inline).

mod event;
mod preset;
mod process;
mod stuck;

pub use event::{Channel, MidiEvent, Note, Velocity};
pub use preset::{
    CcMapEntry, ChannelMapMode, EnginePreset, PortsConfig, PresetError, VelocityConfig,
};
pub use process::{process_event, ProcessOutput, ProcessorChain, VelocityRuntime, MAX_CC_MAP, MAX_OUT};
pub use stuck::ActiveNotes;
