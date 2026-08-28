//! Jambox engine core: wavetable synth, two-operator FM playground, drum voices,
//! FX, and a **sample-accurate** sequencer clock.
//!
//! Architecture law (see `PLAN.md`): the UI is never on the audio or sequencer hot
//! path. This crate has no I/O and no threads. The host binary feeds it timestamped
//! [`Command`]s and calls [`JamboxEngine::render`] from the audio callback.
//!
//! Two properties matter more than features here:
//!
//! 1. **Sample accuracy.** Sequencer events land on an exact frame inside the block,
//!    not on the block boundary, so a loop cannot drift or drop a beat.
//! 2. **No allocation after [`JamboxEngine::new`].** Every buffer is sized up front;
//!    `render` only does arithmetic.

#![forbid(unsafe_code)]

mod clip;
mod command;
mod drums;
mod engine;
mod fm;
mod fx;
mod kaoss;
mod repeat;
mod transport;
mod voice;
mod wavetable;

pub use clip::{Clip, ClipEvent, ClipEventKind, ClipSlot, LaunchMode, Sequencer, MAX_CLIPS};
pub use command::{
    Command, EmitMode, FxParam, FxTarget, ScheduledCommand, SynthParam, MAX_BLOCK_COMMANDS,
};
pub use drums::{drum_model_for_note, DrumKit, DrumModel, DRUM_MODEL_COUNT};
pub use engine::{EngineStatus, JamboxEngine, MidiOutSink, MAX_MIDI_OUT, MAX_RENDER_BLOCK};
pub use fm::{
    clang_index, clang_label, clang_ratio, fm_recipe, FmPatch, FmRecipe, FmSynth, CLANG_LABELS,
    CLANG_RATIOS, FM_RECIPES, FM_RECIPE_COUNT, MAX_FM_VOICES,
};
pub use fx::{FxParams, FxUnit};
pub use kaoss::{
    kaoss_scale, kaoss_scale_index_by_id, migrate_legacy_scale_index, note_at_x, note_index_at_x,
    pack_xy, scale_notes, tone_at_y, unpack_xy, velocity_at_y, KaossMapper, KaossScale,
    LatestTouch, TouchDelta, DEFAULT_KAOSS_SCALE_INDEX, DEFAULT_ROOT_MIDI, KAOSS_SCALES,
    MAX_TOUCH_VOICES, NOTE_NAMES,
};
pub use repeat::{
    RepeatDivision, RepeatEvent, RepeatRack, MAX_REPEAT_EVENTS_PER_BLOCK, MAX_REPEAT_LANES,
};
pub use transport::{Quantize, Transport, PPQ};
pub use voice::{VoicePool, MAX_VOICES};
pub use wavetable::{WaveBank, TABLE_MASK, TABLE_PEAK, TABLE_SIZE};

/// MIDI channel used for drum voices (channel 10, zero-based).
pub const DRUM_CHANNEL: u8 = 9;
