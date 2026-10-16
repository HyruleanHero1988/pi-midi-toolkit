//! Pure MIDI transform logic — no device I/O.
//!
//! Designed for a tight hot path: processors take an event and return zero or more
//! output events without allocating when possible (small fixed buffers / inline).

mod controller;
mod event;
mod ports;
mod preset;
mod process;
mod stuck;

pub use controller::{
    apply_default_overlay_dirs, apply_overlay_dir, builtin_profiles, installed_profiles,
    merge_profile_json, mpk_knob_ccs, mpk_note_for_phrase_cell, mpk_phrase_notes,
    mpk_punch_pad_notes, profile_for_port_name, profile_for_port_name_in, ControllerProfile,
    KnobEncoding,
};
pub use event::{Channel, MidiEvent, Note, Velocity};
pub use ports::{
    cycle_port_filter, is_virtual_port_name, matching_port_names, pick_port_name,
    pick_thru_output_name, port_looks_like_controller_io, port_looks_like_keybed, short_port_label,
};
pub use preset::{
    fanout_dest_mask, fanout_is_identity, format_fanout_targets, toggle_fanout_bit, CcMapEntry,
    ChannelMapMode, EnginePreset, PortsConfig, PresetError, VelocityConfig,
};
pub use process::{
    process_event, ProcessOutput, ProcessorChain, VelocityRuntime, MAX_CC_MAP, MAX_OUT,
};
pub use stuck::ActiveNotes;
