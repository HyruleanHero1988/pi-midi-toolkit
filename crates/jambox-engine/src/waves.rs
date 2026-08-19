//! Load the kiosk wavetable directory into a `WaveBank` (host thread, allocates).
//!
//! Built-ins (sine/square/saw/triangle) stay first so morph indices match the
//! Python UI when a file tries to override those names.

use std::path::Path;

use hound::{SampleFormat, WavReader};
use jambox_core::WaveBank;
use tracing::{info, warn};

const BUILTIN_SKIP: &[&str] = &["sine", "square", "saw", "triangle"];

/// Merge every `*.wav` in `dir` into `bank`. Returns how many files were added.
pub fn load_dir(dir: &Path, bank: &mut WaveBank) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            warn!(%err, path = %dir.display(), "waves: directory unreadable");
            return 0;
        }
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("wav")) == Some(true))
        .collect();
    paths.sort();
    let mut added = 0;
    for path in paths {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if stem.is_empty() || BUILTIN_SKIP.contains(&stem.as_str()) {
            continue;
        }
        match load_mono_cycle(&path) {
            Ok(samples) => {
                bank.insert(&stem, &samples);
                added += 1;
            }
            Err(err) => warn!(%err, file = %path.display(), "waves: skip"),
        }
    }
    info!(dir = %dir.display(), added, total = bank.len(), "waves: bank ready");
    added
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

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{WavSpec, WavWriter};
    use std::f32::consts::TAU;

    #[test]
    fn loads_a_mono_cycle_and_skips_builtins() {
        let dir = tempfile_dir();
        write_sine_wav(&dir.join("warmpad.wav"));
        write_sine_wav(&dir.join("sine.wav"));
        let mut bank = WaveBank::with_builtins();
        let added = load_dir(&dir, &mut bank);
        assert_eq!(added, 1);
        assert!(bank.index_of("warmpad").is_some());
        assert_eq!(bank.index_of("sine"), Some(0));
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("jambox-waves-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_sine_wav(path: &Path) {
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(path, spec).unwrap();
        for i in 0..64 {
            let s = ((i as f32 / 64.0) * TAU).sin();
            writer.write_sample((s * 16000.0) as i16).unwrap();
        }
        writer.finalize().unwrap();
    }
}
