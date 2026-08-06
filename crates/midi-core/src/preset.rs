//! JSON-loadable engine preset (config side; not on the RT hot path).

use serde::{Deserialize, Serialize};

use crate::process::{ProcessorChain, MAX_CC_MAP};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortsConfig {
    /// Substring match against the MIDI input port name.
    pub input: String,
    /// Substring match against the MIDI output port name.
    pub output: String,
}

/// How input channels are rewritten on the thru path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ChannelMapMode {
    /// Leave channels unchanged.
    Identity,
    /// Force every channel voice message onto one output channel (0–15).
    AllTo { channel: u8 },
    /// Per-input-channel remap table (length 16). Index = in channel.
    Remap { map: [u8; 16] },
}

impl Default for ChannelMapMode {
    fn default() -> Self {
        Self::Identity
    }
}

impl ChannelMapMode {
    #[inline]
    pub fn map_channel(&self, in_channel: u8) -> u8 {
        let in_channel = in_channel & 0x0f;
        match self {
            Self::Identity => in_channel,
            Self::AllTo { channel } => *channel & 0x0f,
            Self::Remap { map } => map[in_channel as usize] & 0x0f,
        }
    }
}

/// One CC rewrite rule: `(in_ch, in_cc) → (out_ch, out_cc)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CcMapEntry {
    pub in_channel: u8,
    pub in_cc: u8,
    pub out_channel: u8,
    pub out_cc: u8,
}

/// Velocity transform applied to note-on only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum VelocityConfig {
    PassThrough,
    AlwaysFull,
    Clamp { floor: u8, ceiling: u8 },
    /// Index = input velocity → output velocity (must be length 128).
    Curve { table: Vec<u8> },
}

impl Default for VelocityConfig {
    fn default() -> Self {
        Self::PassThrough
    }
}

impl VelocityConfig {
    #[inline]
    pub fn map(&self, velocity: u8) -> u8 {
        let v = velocity.min(127);
        match self {
            Self::PassThrough => v,
            Self::AlwaysFull => {
                if v == 0 {
                    0
                } else {
                    127
                }
            }
            Self::Clamp { floor, ceiling } => {
                if v == 0 {
                    0
                } else {
                    v.clamp(*floor, (*ceiling).min(127))
                }
            }
            Self::Curve { table } => table.get(v as usize).copied().unwrap_or(v) & 0x7f,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnginePreset {
    pub name: String,
    pub ports: PortsConfig,
    #[serde(default)]
    pub channel_map: ChannelMapMode,
    #[serde(default)]
    pub cc_map: Vec<CcMapEntry>,
    #[serde(default)]
    pub velocity: VelocityConfig,
}

impl EnginePreset {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self, PresetError> {
        let text = std::fs::read_to_string(path.as_ref())?;
        let preset = Self::from_json(&text)?;
        preset.validate()?;
        Ok(preset)
    }

    /// Soft validation for channels / CC ranges / curve length.
    pub fn validate(&self) -> Result<(), PresetError> {
        if self.ports.input.trim().is_empty() || self.ports.output.trim().is_empty() {
            return Err(PresetError::Invalid(
                "ports.input and ports.output must be non-empty".into(),
            ));
        }
        match &self.channel_map {
            ChannelMapMode::AllTo { channel } if *channel > 15 => {
                return Err(PresetError::Invalid(
                    "channel_map all_to.channel must be 0–15".into(),
                ));
            }
            ChannelMapMode::Remap { map } => {
                for (i, ch) in map.iter().enumerate() {
                    if *ch > 15 {
                        return Err(PresetError::Invalid(format!(
                            "channel_map.remap[{i}] must be 0–15"
                        )));
                    }
                }
            }
            _ => {}
        }
        if self.cc_map.len() > MAX_CC_MAP {
            return Err(PresetError::Invalid(format!(
                "cc_map has {} entries; max is {MAX_CC_MAP}",
                self.cc_map.len()
            )));
        }
        for (i, e) in self.cc_map.iter().enumerate() {
            if e.in_channel > 15 || e.out_channel > 15 || e.in_cc > 127 || e.out_cc > 127 {
                return Err(PresetError::Invalid(format!(
                    "cc_map[{i}] out of range (ch 0–15, cc 0–127)"
                )));
            }
        }
        if let VelocityConfig::Clamp { floor, ceiling } = &self.velocity {
            if *floor > 127 || *ceiling > 127 || floor > ceiling {
                return Err(PresetError::Invalid(
                    "velocity clamp: floor/ceiling must be 0–127 and floor <= ceiling".into(),
                ));
            }
        }
        if let VelocityConfig::Curve { table } = &self.velocity {
            if table.len() != 128 {
                return Err(PresetError::Invalid(format!(
                    "velocity curve table must have 128 entries, got {}",
                    table.len()
                )));
            }
        }
        Ok(())
    }

    /// Build the hot-path processor chain from this preset.
    pub fn to_chain(&self) -> ProcessorChain {
        ProcessorChain::from_preset(self)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid preset: {0}")]
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_example_shape() {
        let json = r#"{
            "name": "keys-to-ch3",
            "ports": { "input": "Controller", "output": "Synth" },
            "channel_map": { "mode": "all_to", "channel": 2 },
            "cc_map": [
                { "in_channel": 0, "in_cc": 1, "out_channel": 2, "out_cc": 11 }
            ],
            "velocity": { "mode": "always_full" }
        }"#;
        let p = EnginePreset::from_json(json).unwrap();
        p.validate().unwrap();
        assert_eq!(p.name, "keys-to-ch3");
        assert_eq!(p.channel_map.map_channel(0), 2);
        assert_eq!(p.velocity.map(40), 127);
    }

    #[test]
    fn identity_defaults() {
        let json = r#"{
            "name": "thru",
            "ports": { "input": "in", "output": "out" }
        }"#;
        let p = EnginePreset::from_json(json).unwrap();
        assert!(matches!(p.channel_map, ChannelMapMode::Identity));
        assert!(p.cc_map.is_empty());
        assert!(matches!(p.velocity, VelocityConfig::PassThrough));
    }

    #[test]
    fn reject_bad_channel() {
        let json = r#"{
            "name": "bad",
            "ports": { "input": "in", "output": "out" },
            "channel_map": { "mode": "all_to", "channel": 16 }
        }"#;
        let p = EnginePreset::from_json(json).unwrap();
        assert!(p.validate().is_err());
    }
}
