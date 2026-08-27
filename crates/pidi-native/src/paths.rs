//! Appliance user-data root — songs, pads, settings, presets, user waves, Wi‑Fi.
//!
//! Convention (XDG data home):
//!
//! ```text
//! ${PIDI_DATA_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/pidi}/
//!   settings.json
//!   songs/
//!   phrases/
//!   user-presets/
//!   user-wavetables/
//!   .wifi-credentials
//!   takes/            (optional)
//! ```
//!
//! Code / factory waves / engines stay under `PIDI_REPO_ROOT` (the git tree).
//! Per-subdir env overrides (`PIDI_SONGS_DIR`, …) still win when set.

use std::fs;
use std::path::{Path, PathBuf};

/// Resolve the single user-data root and ensure the standard subdirs exist.
pub fn data_root() -> PathBuf {
    let root = data_root_raw();
    ensure_layout(&root);
    root
}

/// Same as [`data_root`] but without creating directories (for tests / probes).
pub fn data_root_raw() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_DATA_ROOT") {
        let p = p.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        let xdg = xdg.trim();
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("pidi");
        }
    }
    home_dir()
        .map(|h| h.join(".local/share/pidi"))
        .unwrap_or_else(|| PathBuf::from(".local/share/pidi"))
}

pub fn ensure_layout(root: &Path) {
    for sub in ["songs", "phrases", "user-presets", "user-wavetables", "takes"] {
        let _ = fs::create_dir_all(root.join(sub));
    }
}

pub fn songs_dir() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_SONGS_DIR") {
        return PathBuf::from(p);
    }
    data_root().join("songs")
}

pub fn phrases_dir() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_PHRASES_DIR") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("MIDI_TONE_PHRASES_DIR") {
        return PathBuf::from(p);
    }
    data_root().join("phrases")
}

pub fn presets_dir() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_PRESETS_DIR") {
        return PathBuf::from(p);
    }
    data_root().join("user-presets")
}

pub fn settings_path() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_SETTINGS_PATH") {
        return PathBuf::from(p);
    }
    data_root().join("settings.json")
}

pub fn user_wavetables_dir() -> PathBuf {
    if let Ok(p) = std::env::var("JAMBOX_USER_WAVETABLES") {
        return PathBuf::from(p);
    }
    data_root().join("user-wavetables")
}

pub fn wifi_credentials_path() -> PathBuf {
    data_root().join(".wifi-credentials")
}

fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME") {
        if !h.trim().is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    // Windows host tests / rare appliance shells.
    if let Ok(h) = std::env::var("USERPROFILE") {
        if !h.trim().is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn data_root_prefers_pidi_data_root() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("PIDI_DATA_ROOT", "/tmp/pidi-data-test-root");
        std::env::remove_var("XDG_DATA_HOME");
        assert_eq!(data_root_raw(), PathBuf::from("/tmp/pidi-data-test-root"));
        std::env::remove_var("PIDI_DATA_ROOT");
    }

    #[test]
    fn data_root_uses_xdg_when_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("PIDI_DATA_ROOT");
        std::env::set_var("XDG_DATA_HOME", "/tmp/xdg-data");
        assert_eq!(data_root_raw(), PathBuf::from("/tmp/xdg-data/pidi"));
        std::env::remove_var("XDG_DATA_HOME");
    }

    #[test]
    fn subdir_overrides_still_win() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("PIDI_DATA_ROOT", "/tmp/pidi-data-test-root");
        std::env::set_var("PIDI_SONGS_DIR", "/custom/songs");
        assert_eq!(songs_dir(), PathBuf::from("/custom/songs"));
        std::env::remove_var("PIDI_SONGS_DIR");
        std::env::remove_var("PIDI_DATA_ROOT");
    }
}
