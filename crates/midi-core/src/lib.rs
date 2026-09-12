//! Pure MIDI transform logic — no device I/O.
//!
//! Designed for a tight hot path: processors take an event and return zero or more
//! output events without allocating when possible (small fixed buffers / inline).

mod event;
mod ports;
mod preset;
mod process;
mod stuck;

pub use event::{Channel, MidiEvent, Note, Velocity};
pub use ports::{
    cycle_port_filter, is_virtual_port_name, matching_port_names, pick_port_name, short_port_label,
};
pub use preset::{
    fanout_dest_mask, fanout_is_identity, format_fanout_targets, toggle_fanout_bit, CcMapEntry,
    ChannelMapMode, EnginePreset, PortsConfig, PresetError, VelocityConfig,
};
pub use process::{process_event, ProcessOutput, ProcessorChain, VelocityRuntime, MAX_CC_MAP, MAX_OUT};
pub use stuck::ActiveNotes;
