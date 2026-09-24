//! User presets (`user-presets/slot-N.json`) — synth/morph plus pad bank.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::phrases::PhraseFile;

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
    /// Tempo the pad bank was saved at. Needed so second-based events stay in time.
    #[serde(default)]
    pub bpm: Option<f32>,
    /// 16 pad slots. `None` on the vec means an old synth-only preset (leave pads).
    /// `None` inside the vec is an empty pad.
    #[serde(default)]
    pub phrases: Option<Vec<Option<PhraseFile>>>,
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
            bpm: None,
            phrases: None,
        }
    }
}

pub fn presets_dir_from_env() -> PathBuf {
    crate::paths::presets_dir()
}

pub fn slot_path(dir: &Path, slot: usize) -> PathBuf {
    dir.join(format!("slot-{}.json", slot + 1))
}

fn slot_path_padded(dir: &Path, slot: usize) -> PathBuf {
    dir.join(format!("slot-{:02}.json", slot + 1))
}

pub fn load_slot(dir: &Path, slot: usize) -> Option<PresetSnapshot> {
    for path in [slot_path(dir, slot), slot_path_padded(dir, slot)] {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(p) = serde_json::from_str(&raw) {
                return Some(p);
            }
        }
    }
    None
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

    #[test]
    fn old_slot_without_phrases_still_loads() {
        let dir = std::env::temp_dir().join(format!("pidi-preset-old-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            slot_path(&dir, 0),
            r#"{"version":1,"name":"OLD","morph":0.2,"tone":0.3,"level":0.8,"attack":0.1,"release":0.2}"#,
        )
        .unwrap();
        let loaded = load_slot(&dir, 0).unwrap();
        assert_eq!(loaded.name, "OLD");
        assert!(loaded.phrases.is_none());
        assert!(loaded.bpm.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn slot_round_trips_phrases() {
        use crate::phrases::{self, PhrasePad};
        use jambox_protocol::WireClipEvent;

        let dir = std::env::temp_dir().join(format!("pidi-preset-pads-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut pads = std::array::from_fn(|_| PhrasePad::default());
        pads[0] = PhrasePad {
            empty: false,
            loop_mode: true,
            length_ticks: 1920,
            length_secs: 1.0,
            events: vec![WireClipEvent::midi(0, true, 9, 36, 100)],
            ..PhrasePad::default()
        };
        let mut p = PresetSnapshot::default();
        p.version = 3;
        p.name = "KIT".into();
        p.bpm = Some(126.0);
        p.phrases = Some(phrases::snapshot_bank(&pads, 126.0));
        assert!(save_slot(&dir, 1, &p));
        let loaded = load_slot(&dir, 1).unwrap();
        assert_eq!(loaded.name, "KIT");
        assert!((loaded.bpm.unwrap() - 126.0).abs() < 1e-6);
        let bank = phrases::bank_from_snapshot(loaded.phrases.as_deref().unwrap(), 126.0);
        assert!(!bank[0].empty);
        assert!(bank[0].loop_mode);
        assert_eq!(bank[0].events[0].note, 36);
        assert!(bank[1].empty);
        let _ = fs::remove_dir_all(&dir);
    }
}
