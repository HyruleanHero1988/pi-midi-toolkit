//! Versioned, allocation-friendly wire types for jambox clients.
//!
//! The transport is newline-delimited JSON for commissioning compatibility.
//! Semantics matter more than the encoding: edges are reliable and ordered,
//! while native clients coalesce `TouchPhase::Move` before writing them.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
pub const NATIVE_FEATURES: &[&str] = &[
    "touch_sessions",
    "disconnect_release",
    "sample_clock_repeat",
    "runtime_diagnostics",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TouchPhase {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepeatPhase {
    Down,
    Up,
    Cancel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepeatDivision {
    Quarter,
    Eighth,
    EighthTriplet,
    Sixteenth,
    QuarterTriplet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Hello {
        protocol: u16,
        client: String,
        #[serde(default)]
        realtime_owner: bool,
    },
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    AllNotesOff,
    Panic,
    Synth {
        param: String,
        value: f32,
        /// When set, drum_* params apply to that kit model index (0..15).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drum: Option<u8>,
    },
    Fx {
        target: FxTargetSpec,
        param: String,
        value: f32,
    },
    MorphPair {
        a: u16,
        b: u16,
    },
    Tempo {
        bpm: f32,
    },
    BeatsPerBar {
        beats: u8,
    },
    ClipLoad {
        slot: u8,
        #[serde(default)]
        length_ticks: u32,
        #[serde(default)]
        mode: Option<String>,
        events: Vec<WireClipEvent>,
    },
    ClipClear {
        slot: u8,
    },
    ClipMode {
        slot: u8,
        mode: String,
    },
    ClipLaunch {
        slot: u8,
        #[serde(default)]
        quantize: Option<String>,
    },
    ClipStop {
        slot: u8,
        #[serde(default)]
        quantize: Option<String>,
    },
    StopAllClips,
    Status,
    Midi {
        kind: String,
        #[serde(default)]
        channel: u8,
        #[serde(default)]
        note: Option<u8>,
        #[serde(default)]
        velocity: Option<u8>,
        #[serde(default)]
        control: Option<u8>,
        #[serde(default)]
        value: Option<u16>,
    },
    KnobMap {
        mode: String,
        #[serde(default)]
        fx_kind: Option<String>,
        #[serde(default)]
        fx_index: u16,
    },
    /// One KAOSS contact. Native clients guarantee that down/up/cancel are
    /// reliable edges and coalesce move updates by gesture ID.
    Touch {
        gesture: u32,
        phase: TouchPhase,
        x: f32,
        y: f32,
        #[serde(default)]
        channel: u8,
        #[serde(default = "default_velocity")]
        velocity: u8,
    },
    /// A sample-clocked drum repeat owned by one contact.
    Repeat {
        gesture: u32,
        phase: RepeatPhase,
        note: u8,
        #[serde(default = "default_drum_channel")]
        channel: u8,
        #[serde(default = "default_velocity")]
        velocity: u8,
        #[serde(default = "default_repeat_division")]
        division: RepeatDivision,
    },
    /// Rebuild the engine KAOSS note lattice.
    KaossScale {
        #[serde(default)]
        scale_index: u8,
        #[serde(default)]
        key: u8,
        #[serde(default = "default_kaoss_root")]
        root_midi: u8,
        #[serde(default = "default_kaoss_octaves")]
        octaves: u8,
    },
    /// Direct MIDI to the engine's USB/DIN out path (not inject-in).
    MidiEmit {
        kind: String, // "note_on" | "note_off" | "cc"
        channel: u8,
        #[serde(default)]
        note: Option<u8>,
        #[serde(default)]
        velocity: Option<u8>,
        #[serde(default)]
        control: Option<u8>,
        #[serde(default)]
        value: Option<u16>,
    },
    /// Local / Usb / Both for clip playback MIDI+audio and for documenting kaoss
    /// (kaoss routing is mostly UI-driven via MidiEmit + Touch).
    EmitMode {
        target: String, // "clips" | "kaoss"
        mode: String,   // "local" | "usb" | "both"
    },
}

const fn default_velocity() -> u8 {
    110
}

const fn default_drum_channel() -> u8 {
    9
}

const fn default_repeat_division() -> RepeatDivision {
    RepeatDivision::Quarter
}

const fn default_kaoss_root() -> u8 {
    48
}

const fn default_kaoss_octaves() -> u8 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FxTargetSpec {
    Voice { index: u16 },
    Drum { index: u8 },
    DrumGroup,
    Bus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireClipEvent {
    pub tick: u32,
    pub on: bool,
    pub channel: u8,
    pub note: u8,
    #[serde(default)]
    pub velocity: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Ok,
    Hello(HelloReply),
    Error { message: String },
    Status(StatusReply),
    Midi(MidiNotice),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloReply {
    pub protocol: u16,
    pub engine: String,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidiNotice {
    pub kind: String,
    pub channel: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub velocity: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<u16>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct StatusReply {
    pub position: u64,
    pub bpm: f32,
    pub active_voices: u16,
    pub active_drums: u16,
    pub active_repeats: u16,
    pub playing_clips: u16,
    pub peak: f32,
    pub callback_frames: u32,
    pub callback_micros: u32,
    pub callback_peak_micros: u32,
    pub xruns: u64,
    pub command_drops: u64,
    pub emergency_releases: u64,
    pub touch_overwrites: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_round_trips_with_a_stable_gesture() {
        let request = Request::Touch {
            gesture: 42,
            phase: TouchPhase::Move,
            x: 0.25,
            y: 0.75,
            channel: 0,
            velocity: 100,
        };
        let json = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            Request::Touch {
                gesture: 42,
                phase: TouchPhase::Move,
                ..
            }
        ));
    }

    #[test]
    fn repeat_defaults_to_quarter_note_drum_lane() {
        let decoded: Request =
            serde_json::from_str(r#"{"cmd":"repeat","gesture":7,"phase":"down","note":36}"#)
                .unwrap();
        assert!(matches!(
            decoded,
            Request::Repeat {
                division: RepeatDivision::Quarter,
                channel: 9,
                velocity: 110,
                ..
            }
        ));
    }

    #[test]
    fn quarter_triplet_repeat_round_trips() {
        let request = Request::Repeat {
            gesture: 3,
            phase: RepeatPhase::Down,
            note: 36,
            channel: 9,
            velocity: 110,
            division: RepeatDivision::QuarterTriplet,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("quarter_triplet"));
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            Request::Repeat {
                division: RepeatDivision::QuarterTriplet,
                ..
            }
        ));
    }

    #[test]
    fn synth_drum_index_round_trips() {
        let request = Request::Synth {
            param: "drum_tone".into(),
            value: 0.7,
            drum: Some(3),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"drum\":3"));
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, Request::Synth { drum: Some(3), .. }));
        let legacy: Request =
            serde_json::from_str(r#"{"cmd":"synth","param":"drum_tone","value":0.7}"#).unwrap();
        assert!(matches!(legacy, Request::Synth { drum: None, .. }));
    }
}
