//! Session autosave (`settings.json`) — synth + kaoss + tempo snapshot.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::font::FontStyle;

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

    /// Compact label for narrow chrome / pad buttons.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::Usb => "USB",
            Self::Both => "BOTH",
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Usb => "usb",
            Self::Both => "both",
        }
    }

    pub fn includes_local(self) -> bool {
        matches!(self, Self::Local | Self::Both)
    }

    pub fn includes_usb(self) -> bool {
        matches!(self, Self::Usb | Self::Both)
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
    /// On-screen synth keyboard octave relative to C4 (−3..+3).
    #[serde(default)]
    pub synth_octave: i8,
    pub kaoss_scale_index: u8,
    pub kaoss_key: u8,
    /// Left-edge MIDI note of the Kaoss pad (C1..C5).
    #[serde(default = "default_kaoss_root_midi")]
    pub kaoss_root_midi: u8,
    pub kaoss_octaves: u8,
    pub kaoss_program: usize,
    pub kaoss_gate: usize,
    pub kaoss_hold: bool,
    pub fx_bus: [f32; 3],
    /// Flanger wet (4th Settings slider). Split from `fx_bus` so old sessions still parse.
    #[serde(default)]
    pub fx_flanger: f32,
    #[serde(default)]
    pub kaoss_show_all: bool,
    #[serde(default)]
    pub kaoss_channel: u8,
    #[serde(default)]
    pub vibrato_always: f32,
    #[serde(default = "default_vibrato_depth")]
    pub vibrato_depth: f32,
    #[serde(default = "default_vibrato_rate")]
    pub vibrato_rate: f32,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub pads_out: OutMode,
    #[serde(default)]
    pub song_out: OutMode,
    #[serde(default = "default_kaoss_out")]
    pub kaoss_out: OutMode,
    #[serde(default)]
    pub chords_out: OutMode,
    #[serde(default)]
    pub chords_hold: bool,
    #[serde(default)]
    pub chords_key: u8,
    #[serde(default)]
    pub font_style: FontStyle,
    #[serde(default = "default_screensaver_sec")]
    pub screensaver_sec: f32,
    /// Pad visualizer: "cells" | "glow" (also accepts legacy "rainbow" | "mono").
    #[serde(default = "default_kaoss_viz_style")]
    pub kaoss_viz_style: String,
    /// Index into pad color palette (0 = RAINBOW, then solids). Legacy mono
    /// sessions used 0 = PINK — migrated on load when style was `"mono"`.
    #[serde(default)]
    pub kaoss_mono_color: usize,
}

fn default_kaoss_root_midi() -> u8 {
    jambox_core::DEFAULT_ROOT_MIDI
}

fn default_screensaver_sec() -> f32 {
    crate::screensaver::DEFAULT_TIMEOUT_SEC
}

fn default_kaoss_out() -> OutMode {
    OutMode::Local
}

fn default_kaoss_viz_style() -> String {
    "cells".into()
}

fn default_vibrato_depth() -> f32 {
    0.5
}

fn default_vibrato_rate() -> f32 {
    5.0
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            version: 3,
            bpm: 120.0,
            morph: 0.5,
            tone: 0.5,
            level: 0.8,
            attack: 0.05,
            release: 0.3,
            morph_a: 0,
            morph_b: 1,
            synth_octave: 0,
            kaoss_scale_index: jambox_core::DEFAULT_KAOSS_SCALE_INDEX,
            kaoss_key: 0,
            kaoss_root_midi: default_kaoss_root_midi(),
            kaoss_octaves: 2,
            kaoss_program: 0,
            kaoss_gate: 0,
            kaoss_hold: false,
            fx_bus: [0.0, 0.0, 0.0],
            fx_flanger: 0.0,
            kaoss_show_all: false,
            kaoss_channel: 0,
            vibrato_always: 0.0,
            vibrato_depth: default_vibrato_depth(),
            vibrato_rate: default_vibrato_rate(),
            mode: "kaoss".into(),
            pads_out: OutMode::Both,
            song_out: OutMode::Both,
            kaoss_out: OutMode::Local,
            chords_out: OutMode::Both,
            chords_hold: true,
            chords_key: 0,
            font_style: FontStyle::Retro,
            screensaver_sec: crate::screensaver::DEFAULT_TIMEOUT_SEC,
            kaoss_viz_style: default_kaoss_viz_style(),
            kaoss_mono_color: 0,
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
        assert_eq!(s.kaoss_out, OutMode::Local);
        assert_eq!(s.font_style, FontStyle::Retro);
    }
}
