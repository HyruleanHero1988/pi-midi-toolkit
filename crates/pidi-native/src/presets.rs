//! User presets (`user-presets/slot-N.json`) — synth/morph snapshot.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetSnapshot {
    pub version: u32,
    pub name: String,
    pub morph: f32,
    pub tone: f32,
    pub level: f32,
    pub attack: f32,
    pub release: f32,
    #[serde(default)]
    pub morph_a: u16,
    #[serde(default = "default_morph_b")]
    pub morph_b: u16,
}

fn default_morph_b() -> u16 {
    1
}

impl Default for PresetSnapshot {
    fn default() -> Self {
        Self {
            version: 1,
            name: "INIT".into(),
            morph: 0.5,
            tone: 0.5,
            level: 0.8,
            attack: 0.05,
            release: 0.3,
            morph_a: 0,
            morph_b: 1,
        }
    }
}

pub fn presets_dir_from_env() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_PRESETS_DIR") {
        return PathBuf::from(p);
    }
    PathBuf::from("user-presets")
}

pub fn slot_path(dir: &Path, slot: usize) -> PathBuf {
    dir.join(format!("slot-{}.json", slot + 1))
}

pub fn load_slot(dir: &Path, slot: usize) -> Option<PresetSnapshot> {
    let path = slot_path(dir, slot);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_slot(dir: &Path, slot: usize, preset: &PresetSnapshot) -> bool {
    if let Err(err) = fs::create_dir_all(dir) {
        tracing::warn!(%err, "presets: mkdir failed");
        return false;
    }
    let path = slot_path(dir, slot);
    match serde_json::to_string_pretty(preset) {
        Ok(body) => fs::write(path, body).is_ok(),
        Err(_) => false,
    }
}

pub fn delete_slot(dir: &Path, slot: usize) -> bool {
    let path = slot_path(dir, slot);
    if path.is_file() {
        fs::remove_file(path).is_ok()
    } else {
        true
    }
}

pub fn list_occupied(dir: &Path) -> [bool; 8] {
    let mut out = [false; 8];
    for i in 0..8 {
        out[i] = slot_path(dir, i).is_file();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_slot() {
        let dir = std::env::temp_dir().join(format!("pidi-preset-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut p = PresetSnapshot::default();
        p.name = "TEST".into();
        p.morph = 0.25;
        assert!(save_slot(&dir, 0, &p));
        let loaded = load_slot(&dir, 0).unwrap();
        assert_eq!(loaded.name, "TEST");
        assert!((loaded.morph - 0.25).abs() < 1e-6);
        let _ = fs::remove_dir_all(&dir);
    }
}
