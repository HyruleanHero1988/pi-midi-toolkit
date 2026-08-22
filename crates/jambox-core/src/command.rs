//! Commands the UI sends to the audio thread.
//!
//! Every variant is `Copy` and free of heap data so it can cross a lock-free ring
//! without allocating. A command carries the frame it should take effect on, which
//! is what lets a knob move or a pad hit land mid-block instead of being smeared to
//! the next buffer boundary.

use crate::clip::LaunchMode;
use crate::repeat::RepeatDivision;
use crate::transport::Quantize;

/// Commands applied per block. Anything beyond this in one block is applied at the
/// block end rather than dropped.
pub const MAX_BLOCK_COMMANDS: usize = 256;

/// Which FX insert a change is aimed at (see `PLAN.md` FX routing table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxTarget {
    /// One wavetable voice, by bank index.
    Voice(u16),
    /// One drum model, by kit index.
    Drum(u8),
    /// The shared "all drums" kit bus.
    DrumGroup,
    /// Master mix bus.
    Bus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxParam {
    Drive,
    DelayTime,
    DelayFb,
    DelayMix,
    ReverbSize,
    ReverbMix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthParam {
    Morph,
    Tone,
    Level,
    Attack,
    Release,
    VibratoDepth,
    VibratoRate,
    /// 0..1 mod-wheel amount. Vibrato depth is scaled by max(this, VibratoAlways).
    VibratoMod,
    /// 0..1 always-on vibrato amount (Kaoss VIB). Combined with the mod wheel.
    VibratoAlways,
    PitchBend,
    DrumPitch,
    DrumDecay,
    DrumNoise,
    DrumTone,
    DrumLevel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    /// Release everything with its normal tail.
    AllNotesOff,
    /// Hard stop: kill voices, clips, and FX tails.
    Panic,
    SetSynth {
        param: SynthParam,
        value: f32,
    },
    SetFx {
        target: FxTarget,
        param: FxParam,
        value: f32,
    },
    SetMorphPair {
        a: u16,
        b: u16,
    },
    SetTempo {
        bpm: f32,
    },
    SetBeatsPerBar {
        beats: u8,
    },
    LaunchClip {
        slot: u8,
        quantize: Quantize,
    },
    StopClip {
        slot: u8,
        quantize: Quantize,
    },
    StopAllClips,
    SetClipMode {
        slot: u8,
        mode: LaunchMode,
    },
    StartRepeat {
        owner: u32,
        channel: u8,
        note: u8,
        velocity: u8,
        division: RepeatDivision,
    },
    StopRepeat {
        owner: u32,
    },
    StopAllRepeats,
    /// Internal event produced by the repeat rack. Keeping the owner attached
    /// lets a stop command earlier in the same block invalidate a queued hit.
    RepeatHit {
        owner: u32,
        channel: u8,
        note: u8,
        velocity: u8,
    },
    /// KAOSS contact down. `x`/`y` are 0..65535 so the command stays `Copy`.
    TouchDown {
        owner: u32,
        x: u16,
        y: u16,
        channel: u8,
        velocity: u8,
    },
    TouchUp {
        owner: u32,
    },
    TouchCancel {
        owner: u32,
    },
}

/// A command plus the frame within the block where it takes effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduledCommand {
    pub frame: u32,
    pub command: Command,
}

impl ScheduledCommand {
    pub fn now(command: Command) -> Self {
        Self { frame: 0, command }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_plain_copy_data() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<Command>();
        assert_copy::<ScheduledCommand>();
    }

    #[test]
    fn command_stays_small_enough_for_a_ring() {
        // Guard against someone adding a String or Vec to the hot-path type.
        // TouchDown packs two u16 coordinates; keep this well under a cache line
        // so a String/Vec cannot sneak onto the audio ring.
        assert!(std::mem::size_of::<Command>() <= 24);
    }
}
