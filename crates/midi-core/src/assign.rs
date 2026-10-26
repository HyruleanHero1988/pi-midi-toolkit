//! Live remaps for the few expression controls every controller has.
//!
//! Knobs already follow the current page (keys / kit / FM / FX). The joystick
//! and pad aftertouch do not — they stay assigned until the user changes them
//! on SET → MIDI.

use serde::{Deserialize, Serialize};

/// Hardware expression source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiSource {
    StickX,
    StickY,
    Aftertouch,
}

impl MidiSource {
    pub const ALL: [Self; 3] = [Self::StickX, Self::StickY, Self::Aftertouch];

    pub fn label(self) -> &'static str {
        match self {
            Self::StickX => "STICK X",
            Self::StickY => "STICK Y",
            Self::Aftertouch => "AFTERTOUCH",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::StickX => "stick X",
            Self::StickY => "stick Y",
            Self::Aftertouch => "aftertouch",
        }
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }
}

/// What an expression source writes into the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiDest {
    Off,
    Vibrato,
    Pitch,
    Tone,
    Morph,
    Level,
    DrumTone,
    DrumPitch,
    DrumDecay,
    DrumNoise,
    DrumLevel,
}

impl MidiDest {
    pub const ALL: [Self; 11] = [
        Self::Pitch,
        Self::Vibrato,
        Self::Tone,
        Self::Morph,
        Self::Level,
        Self::DrumTone,
        Self::DrumPitch,
        Self::DrumDecay,
        Self::DrumNoise,
        Self::DrumLevel,
        Self::Off,
    ];

    pub fn default_for(source: MidiSource) -> Self {
        match source {
            MidiSource::StickX => Self::Pitch,
            MidiSource::StickY => Self::Vibrato,
            MidiSource::Aftertouch => Self::DrumTone,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Vibrato => "VIB",
            Self::Pitch => "PITCH",
            Self::Tone => "TONE",
            Self::Morph => "MORPH",
            Self::Level => "LEVEL",
            Self::DrumTone => "D TONE",
            Self::DrumPitch => "D PIT",
            Self::DrumDecay => "D DEC",
            Self::DrumNoise => "D NOIZ",
            Self::DrumLevel => "D LVL",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Vibrato => "vibrato",
            Self::Pitch => "pitch",
            Self::Tone => "tone",
            Self::Morph => "morph",
            Self::Level => "level",
            Self::DrumTone => "drum tone",
            Self::DrumPitch => "drum pitch",
            Self::DrumDecay => "drum decay",
            Self::DrumNoise => "drum noise",
            Self::DrumLevel => "drum level",
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Vibrato => 1,
            Self::Pitch => 2,
            Self::Tone => 3,
            Self::Morph => 4,
            Self::Level => 5,
            Self::DrumTone => 6,
            Self::DrumPitch => 7,
            Self::DrumDecay => 8,
            Self::DrumNoise => 9,
            Self::DrumLevel => 10,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Vibrato,
            2 => Self::Pitch,
            3 => Self::Tone,
            4 => Self::Morph,
            5 => Self::Level,
            6 => Self::DrumTone,
            7 => Self::DrumPitch,
            8 => Self::DrumDecay,
            9 => Self::DrumNoise,
            10 => Self::DrumLevel,
            _ => Self::Off,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Vibrato => "vibrato",
            Self::Pitch => "pitch",
            Self::Tone => "tone",
            Self::Morph => "morph",
            Self::Level => "level",
            Self::DrumTone => "drum_tone",
            Self::DrumPitch => "drum_pitch",
            Self::DrumDecay => "drum_decay",
            Self::DrumNoise => "drum_noise",
            Self::DrumLevel => "drum_level",
        }
    }

    pub fn from_id(name: &str) -> Option<Self> {
        Some(match name {
            "off" => Self::Off,
            "vibrato" | "vib" => Self::Vibrato,
            "pitch" | "pitch_bend" => Self::Pitch,
            "tone" => Self::Tone,
            "morph" => Self::Morph,
            "level" => Self::Level,
            "drum_tone" => Self::DrumTone,
            "drum_pitch" => Self::DrumPitch,
            "drum_decay" => Self::DrumDecay,
            "drum_noise" => Self::DrumNoise,
            "drum_level" => Self::DrumLevel,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_defaults_match_the_old_hard_wires() {
        assert_eq!(MidiDest::default_for(MidiSource::StickX), MidiDest::Pitch);
        assert_eq!(MidiDest::default_for(MidiSource::StickY), MidiDest::Vibrato);
        assert_eq!(
            MidiDest::default_for(MidiSource::Aftertouch),
            MidiDest::DrumTone
        );
    }

    #[test]
    fn dest_bytes_round_trip() {
        for dest in MidiDest::ALL {
            assert_eq!(MidiDest::from_u8(dest.as_u8()), dest);
        }
    }
}
