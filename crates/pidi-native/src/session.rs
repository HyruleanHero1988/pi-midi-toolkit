//! Session autosave (`settings.json`) — synth + kaoss + tempo snapshot.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Stored MIDI/audio routing preference (engine may still always play local).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutMode {
    Local,
    Usb,
    #[default]
    Both,
}

impl OutMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Local => Self::Usb,
            Self::Usb => Self::Both,
            Self::Both => Self::Local,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "OUT: LOCAL",
            Self::Usb => "OUT: USB",
            Self::Both => "OUT: BOTH",
        }
    }

    pub fn color(self) -> u32 {
        match self {
            Self::Local => 0x3c3836,
            Self::Usb => 0x458588,
            Self::Both => 0x689d6a,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub version: u32,
    pub bpm: f32,
    pub morph: f32,
    pub tone: f32,
    pub level: f32,
    pub attack: f32,
    pub release: f32,
    pub morph_a: u16,
    pub morph_b: u16,
    pub kaoss_scale_index: u8,
    pub kaoss_key: u8,
    pub kaoss_octaves: u8,
    pub kaoss_program: usize,
    pub kaoss_gate: usize,
    pub kaoss_hold: bool,
    pub fx_bus: [f32; 3],
    #[serde(default)]
    pub kaoss_show_all: bool,
    #[serde(default)]
    pub kaoss_channel: u8,
    #[serde(default)]
    pub vibrato_always: f32,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub pads_out: OutMode,
    #[serde(default)]
    pub song_out: OutMode,
    #[serde(default)]
    pub kaoss_out: OutMode,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            version: 1,
            bpm: 120.0,
            morph: 0.5,
            tone: 0.5,
            level: 0.8,
            attack: 0.05,
            release: 0.3,
            morph_a: 0,
            morph_b: 1,
            kaoss_scale_index: 1,
            kaoss_key: 0,
            kaoss_octaves: 2,
            kaoss_program: 0,
            kaoss_gate: 0,
            kaoss_hold: false,
            fx_bus: [0.0, 0.0, 0.0],
            kaoss_show_all: false,
            kaoss_channel: 0,
            vibrato_always: 0.0,
            mode: "kaoss".into(),
            pads_out: OutMode::Both,
            song_out: OutMode::Both,
            kaoss_out: OutMode::Both,
        }
    }
}

pub fn session_path_from_env() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_SETTINGS_PATH") {
        return PathBuf::from(p);
    }
    PathBuf::from("settings.json")
}

pub fn load(path: &Path) -> Option<SessionState> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save(path: &Path, state: &SessionState) -> bool {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    match serde_json::to_string_pretty(state) {
        Ok(body) => fs::write(path, body + "\n").is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_mode_cycles() {
        assert_eq!(OutMode::Local.cycle(), OutMode::Usb);
        assert_eq!(OutMode::Usb.cycle(), OutMode::Both);
        assert_eq!(OutMode::Both.cycle(), OutMode::Local);
    }

    #[test]
    fn session_roundtrip_includes_out_modes() {
        let mut s = SessionState::default();
        s.pads_out = OutMode::Usb;
        s.song_out = OutMode::Local;
        s.kaoss_out = OutMode::Both;
        let json = serde_json::to_string(&s).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pads_out, OutMode::Usb);
        assert_eq!(back.song_out, OutMode::Local);
        assert_eq!(back.kaoss_out, OutMode::Both);
    }

    #[test]
    fn old_session_json_defaults_out_modes() {
        let json = r#"{"version":1,"bpm":120.0,"morph":0.5,"tone":0.5,"level":0.8,"attack":0.05,"release":0.3,"morph_a":0,"morph_b":1,"kaoss_scale_index":1,"kaoss_key":0,"kaoss_octaves":2,"kaoss_program":0,"kaoss_gate":0,"kaoss_hold":false,"fx_bus":[0.0,0.0,0.0]}"#;
        let s: SessionState = serde_json::from_str(json).unwrap();
        assert_eq!(s.pads_out, OutMode::Both);
        assert_eq!(s.song_out, OutMode::Both);
        assert_eq!(s.kaoss_out, OutMode::Both);
    }
}
