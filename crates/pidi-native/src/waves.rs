//! Wavetable name catalog + scope bank for the native morph picker.
//!
//! Indices must match jambox-engine's `WaveBank` load order: built-ins first,
//! then sorted `*.wav` stems from the waves dirs (skipping builtin overrides).

use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader};
use jambox_core::WaveBank;

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
        "apps/pidi/user-wavetables",
        "wavetables",
        "user-wavetables",
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
        append_names(dir, &mut names);
    }
    names
}

/// Scope / SAVE-AS bank: builtins + every wavetable file the picker lists.
///
/// Same order as `jambox-engine` (`with_builtins` then each waves dir). Without
/// this, morph A/B indices clamp into the four builtins and the CRT scope
/// never follows the selected voices.
pub fn load_wave_bank(dirs: &[PathBuf]) -> WaveBank {
    let mut bank = WaveBank::with_builtins();
    for dir in dirs {
        load_dir_into(dir, &mut bank);
    }
    bank
}

fn append_names(dir: &Path, names: &mut Vec<String>) {
    for path in sorted_wavs(dir) {
        let Some(stem) = wav_stem(&path) else {
            continue;
        };
        if BUILTINS.contains(&stem.as_str()) {
            continue;
        }
        if names.iter().any(|n| n == &stem) {
            // Later dirs replace samples in the bank, but keep one catalog slot.
            continue;
        }
        names.push(stem);
    }
}

fn load_dir_into(dir: &Path, bank: &mut WaveBank) {
    for path in sorted_wavs(dir) {
        let Some(stem) = wav_stem(&path) else {
            continue;
        };
        if BUILTINS.contains(&stem.as_str()) {
            continue;
        }
        match load_mono_cycle(&path) {
            Ok(samples) => {
                bank.insert(&stem, &samples);
            }
            Err(_) => {
                // Skip unreadable / empty files; picker may still list the stem.
            }
        }
    }
}

fn sorted_wavs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
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
    paths
}

fn wav_stem(path: &Path) -> Option<String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())?
        .to_ascii_lowercase();
    if stem.is_empty() {
        None
    } else {
        Some(stem)
    }
}

fn load_mono_cycle(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let mut mono = Vec::new();
    match spec.sample_format {
        SampleFormat::Float => {
            for (i, sample) in reader.samples::<f32>().enumerate() {
                let v = sample.map_err(|e| e.to_string())?;
                if i % channels == 0 {
                    mono.push(v);
                }
            }
        }
        SampleFormat::Int => {
            let max = match spec.bits_per_sample {
                8 => 128.0,
                16 => 32768.0,
                24 => 8388608.0,
                32 => 2147483648.0,
                n => (1u32 << (n.saturating_sub(1).min(31))) as f32,
            };
            for (i, sample) in reader.samples::<i32>().enumerate() {
                let v = sample.map_err(|e| e.to_string())? as f32 / max;
                if i % channels == 0 {
                    mono.push(v);
                }
            }
        }
    }
    if mono.is_empty() {
        return Err("empty wav".into());
    }
    Ok(mono)
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
    use hound::{SampleFormat, WavSpec, WavWriter};
    use std::f32::consts::TAU;

    #[test]
    fn builtins_lead_the_catalog() {
        let names = list_wave_names(&[]);
        assert_eq!(&names[..4], &["sine", "square", "saw", "triangle"]);
    }

    #[test]
    fn loaded_bank_keeps_file_voices_for_scope_indices() {
        let dir = tempfile_dir();
        write_sine_wav(&dir.join("warmpad.wav"));
        write_pulse_wav(&dir.join("narrow.wav"));
        let names = list_wave_names(&[dir.clone()]);
        let mut bank = load_wave_bank(&[dir]);
        assert!(names.len() >= 6, "catalog should list file voices");
        assert_eq!(bank.len(), names.len());
        let warm = bank.index_of("warmpad").expect("warmpad");
        let narrow = bank.index_of("narrow").expect("narrow");
        assert!(warm >= 4 && narrow >= 4);
        bank.set_morph_pair(warm, narrow);
        bank.set_morph(0.0);
        bank.rebuild_morph();
        let (a, b, _) = bank.morph_pair();
        assert_eq!((a, b), (warm, narrow), "scope must not clamp into builtins");
        assert!((bank.morph_table()[128] - bank.table(warm)[128]).abs() < 1e-5);
        bank.set_morph(1.0);
        bank.rebuild_morph();
        assert!((bank.morph_table()[128] - bank.table(narrow)[128]).abs() < 1e-5);
    }

    fn tempfile_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "pidi-waves-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_sine_wav(path: &Path) {
        write_cycle(path, |i, n| ((i as f32 / n as f32) * TAU).sin());
    }

    fn write_pulse_wav(path: &Path) {
        write_cycle(path, |i, n| if i * 4 < n { 0.8 } else { -0.8 });
    }

    fn write_cycle(path: &Path, sample: impl Fn(usize, usize) -> f32) {
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(path, spec).unwrap();
        let n = 64;
        for i in 0..n {
            writer
                .write_sample((sample(i, n) * 16000.0) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();
    }
}
