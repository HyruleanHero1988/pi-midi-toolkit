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

/// How a stick axis turns its travel into a 0..1 amount.
///
/// - `Bipolar` (full range): the middle of the travel is the neutral point and
///   the whole travel is used, so a target like PITCH or TONE sits at its
///   neutral value at rest, goes down one way and up the other.
/// - `Distance` (unipolar): the amount is how far the stick is from rest.
///   Rest is 0 and either direction raises it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StickRange {
    Bipolar,
    Distance,
}

/// Wheel noise at rest must not read as a deflection in distance mode.
pub const STICK_DISTANCE_DEADZONE: f32 = 0.02;

impl StickRange {
    /// Factory behavior: the pitch wheel (X) is full range, CC1 (Y) is distance.
    pub fn default_for(source: MidiSource) -> Self {
        match source {
            MidiSource::StickX => Self::Bipolar,
            MidiSource::StickY | MidiSource::Aftertouch => Self::Distance,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Bipolar => Self::Distance,
            Self::Distance => Self::Bipolar,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bipolar => "BIPOLAR",
            Self::Distance => "DISTANCE",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Bipolar => "bipolar (full range)",
            Self::Distance => "distance from center",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Bipolar => "bipolar",
            Self::Distance => "distance",
        }
    }

    pub fn from_id(name: &str) -> Option<Self> {
        Some(match name {
            "bipolar" | "full" => Self::Bipolar,
            "distance" | "unipolar" => Self::Distance,
            _ => return None,
        })
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Self::Bipolar => 0,
            Self::Distance => 1,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        if value == 1 {
            Self::Distance
        } else {
            Self::Bipolar
        }
    }
}

/// Joystick X is the 14-bit pitch wheel (0..16383, rest 8192). Returns the
/// 0..1 amount and the pitch-bend value in semitones (+-2 full travel).
pub fn stick_x_amount(wheel: u16, range: StickRange) -> (f32, f32) {
    let s = ((wheel as f32 - 8192.0) / 8192.0).clamp(-1.0, 1.0);
    match range {
        StickRange::Bipolar => (0.5 + 0.5 * s, s * 2.0),
        StickRange::Distance => {
            let d = s.abs();
            let d = if d <= STICK_DISTANCE_DEADZONE { 0.0 } else { d };
            (d, d * 2.0)
        }
    }
}

/// Joystick Y is CC1 (`up`, rest 0). Some controllers send the other half of
/// the travel on a second CC (`down`, rest 0).
///
/// `Distance` is the raw CC1 value: rest 0, more travel = more amount.
/// `Bipolar` puts rest at 0.5 (the neutral of TONE, MORPH, PITCH...), `up`
/// moves toward 1 and `down`, when the controller sends it, toward 0.
pub fn stick_y_amount(up: u8, down: u8, range: StickRange) -> f32 {
    match range {
        StickRange::Distance => (up as f32 / 127.0).clamp(0.0, 1.0),
        StickRange::Bipolar => {
            let s = ((up as f32 - down as f32) / 127.0).clamp(-1.0, 1.0);
            0.5 + 0.5 * s
        }
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

    #[test]
    fn range_defaults_keep_the_old_behavior() {
        assert_eq!(StickRange::default_for(MidiSource::StickX), StickRange::Bipolar);
        assert_eq!(StickRange::default_for(MidiSource::StickY), StickRange::Distance);
    }

    #[test]
    fn range_ids_and_bytes_round_trip() {
        for r in [StickRange::Bipolar, StickRange::Distance] {
            assert_eq!(StickRange::from_id(r.id()), Some(r));
            assert_eq!(StickRange::from_u8(r.as_u8()), r);
            assert_eq!(r.toggled().toggled(), r);
        }
        assert_eq!(StickRange::from_id("nope"), None);
    }

    #[test]
    fn x_bipolar_centers_on_neutral_and_uses_the_whole_travel() {
        let (u, semis) = stick_x_amount(8192, StickRange::Bipolar);
        assert!((u - 0.5).abs() < 1e-6 && semis.abs() < 1e-6);
        let (lo, lo_semis) = stick_x_amount(0, StickRange::Bipolar);
        let (hi, hi_semis) = stick_x_amount(16383, StickRange::Bipolar);
        assert!(lo < 0.001 && hi > 0.999);
        assert!((lo_semis + 2.0).abs() < 1e-3 && (hi_semis - 2.0).abs() < 1e-3);
    }

    #[test]
    fn x_distance_is_zero_at_rest_and_rises_both_ways() {
        assert_eq!(stick_x_amount(8192, StickRange::Distance).0, 0.0);
        assert_eq!(stick_x_amount(8192 + 50, StickRange::Distance).0, 0.0);
        let left = stick_x_amount(0, StickRange::Distance).0;
        let right = stick_x_amount(16383, StickRange::Distance).0;
        assert!(left > 0.99 && right > 0.99);
        assert!(stick_x_amount(4096, StickRange::Distance).0 > 0.4);
    }

    #[test]
    fn y_distance_is_the_raw_cc1_value() {
        assert_eq!(stick_y_amount(0, 0, StickRange::Distance), 0.0);
        assert_eq!(stick_y_amount(127, 0, StickRange::Distance), 1.0);
        assert_eq!(stick_y_amount(127, 127, StickRange::Distance), 1.0);
    }

    #[test]
    fn y_bipolar_rests_on_neutral_not_zero() {
        assert_eq!(stick_y_amount(0, 0, StickRange::Bipolar), 0.5);
        assert_eq!(stick_y_amount(127, 0, StickRange::Bipolar), 1.0);
        assert_eq!(stick_y_amount(0, 127, StickRange::Bipolar), 0.0);
        assert_eq!(stick_y_amount(127, 127, StickRange::Bipolar), 0.5);
    }
}
