//! Wavetable name catalog for the native morph picker.
//!
//! Indices must match jambox-engine's `WaveBank` load order: built-ins first,
//! then sorted `*.wav` stems from the waves dirs (skipping builtin overrides).

use std::path::{Path, PathBuf};

const BUILTINS: &[&str] = &["sine", "square", "saw", "triangle"];

pub fn waves_dirs_from_env() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(p) = std::env::var("JAMBOX_WAVETABLES") {
        dirs.push(PathBuf::from(p));
    }
    if let Ok(p) = std::env::var("JAMBOX_USER_WAVETABLES") {
        dirs.push(PathBuf::from(p));
    }
    // Lab / appliance defaults (same layout as the Tk kiosk tree).
    for candidate in [
        "apps/pidi/wavetables",
        "wavetables",
        "/home/ray/pi-midi-toolkit/apps/pidi/wavetables",
        "/home/ray/pi-midi-toolkit/apps/pidi/user-wavetables",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_dir() && !dirs.iter().any(|d| d == &p) {
            dirs.push(p);
        }
    }
    dirs
}

pub fn list_wave_names(dirs: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = BUILTINS.iter().map(|s| (*s).to_string()).collect();
    for dir in dirs {
        append_dir(dir, &mut names);
    }
    names
}

fn append_dir(dir: &Path, names: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("wav"))
                == Some(true)
        })
        .collect();
    paths.sort();
    for path in paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if stem.is_empty() || BUILTINS.contains(&stem.as_str()) {
            continue;
        }
        if let Some(existing) = names.iter().position(|n| n == &stem) {
            // Later dirs replace earlier file tables (engine insert semantics).
            let _ = existing;
            continue;
        }
        names.push(stem);
    }
}

pub fn short_label(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    if upper.len() <= 10 {
        upper
    } else {
        format!("{}…", &upper[..9])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_lead_the_catalog() {
        let names = list_wave_names(&[]);
        assert_eq!(&names[..4], &["sine", "square", "saw", "triangle"]);
    }
}
