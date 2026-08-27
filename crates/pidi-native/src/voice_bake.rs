//! Bake the live morph (+ optional drive/tone) into a user wavetable WAV + FX sidecar.

use jambox_core::{WaveBank, TABLE_PEAK, TABLE_SIZE};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const VOICE_NAME_MAX: usize = 24;
const BUILTINS: &[&str] = &["sine", "square", "saw", "triangle"];
const SAMPLE_RATE: u32 = 44100;

/// Resolve `user-wavetables/` (env override, then common appliance paths).
pub fn user_wavetables_dir() -> PathBuf {
    if let Ok(p) = std::env::var("JAMBOX_USER_WAVETABLES") {
        let path = PathBuf::from(p);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    for candidate in [
        "apps/pidi/user-wavetables",
        "user-wavetables",
        "/home/ray/pi-midi-toolkit/apps/pidi/user-wavetables",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_dir() || p.parent().map(|par| par.is_dir()).unwrap_or(false) {
            return p;
        }
    }
    PathBuf::from("user-wavetables")
}

pub fn sanitize_voice_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(VOICE_NAME_MAX));
    let mut prev_us = false;
    for ch in raw.chars() {
        let c = ch.to_ascii_lowercase();
        let ok = c.is_ascii_alphanumeric() || c == '_';
        if ok {
            out.push(c);
            prev_us = c == '_';
        } else if !prev_us && !out.is_empty() {
            out.push('_');
            prev_us = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("voice");
    }
    out.truncate(VOICE_NAME_MAX);
    out
}

pub fn suggest_voice_name(name_a: &str, name_b: &str, morph: f32) -> String {
    let a = sanitize_voice_name(name_a);
    let b = sanitize_voice_name(name_b);
    let pct = (morph.clamp(0.0, 1.0) * 100.0).round() as i32;
    if a == b {
        sanitize_voice_name(&format!("{a}_saved"))
    } else {
        sanitize_voice_name(&format!("{a}_{b}_{pct}"))
    }
}

pub fn unique_voice_name(base: &str, existing: &[String]) -> String {
    let key = sanitize_voice_name(base);
    let taken = |k: &str| BUILTINS.contains(&k) || existing.iter().any(|e| e == k);
    if !taken(&key) {
        return key;
    }
    for n in 2..1000 {
        let suffix = format!("_{n}");
        let stem_len = VOICE_NAME_MAX.saturating_sub(suffix.len()).max(1);
        let cand = format!("{}{suffix}", &key[..key.len().min(stem_len)]);
        let cand = sanitize_voice_name(&cand);
        if !taken(&cand) {
            return cand;
        }
    }
    sanitize_voice_name(&format!(
        "voice_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() % 100_000)
            .unwrap_or(0)
    ))
}

/// Soft-clip drive matching Python bake (`tanh` amount = 1 + drive*12).
pub fn apply_drive(samples: &mut [f32], drive: f32) {
    if drive <= 0.001 {
        return;
    }
    let amount = 1.0 + drive * 12.0;
    let norm = amount.tanh().max(0.25);
    for s in samples.iter_mut() {
        *s = (*s * amount).tanh() / norm;
    }
}

/// Tone as circular moving-average (darker = wider window), matching Python.
pub fn apply_tone(samples: &mut [f32], tone: f32) {
    if tone >= 0.999 {
        return;
    }
    let win = ((1.0 - tone) * 48.0).round() as usize;
    if win <= 1 {
        return;
    }
    circular_moving_average(samples, win);
}

fn circular_moving_average(x: &mut [f32], win: usize) {
    let n = x.len();
    if n == 0 || win <= 1 {
        return;
    }
    let mut acc = 0.0f32;
    let mut buf = vec![0.0f32; n];
    for i in 0..win {
        acc += x[i % n];
    }
    for i in 0..n {
        buf[i] = acc / win as f32;
        acc -= x[i % n];
        acc += x[(i + win) % n];
    }
    x.copy_from_slice(&buf);
}

fn normalize_peak(samples: &mut [f32]) {
    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    let scale = TABLE_PEAK / peak;
    for s in samples.iter_mut() {
        *s *= scale;
    }
}

/// Bake morph table + drive/tone into a `TABLE_SIZE` cycle.
pub fn bake_cycle(bank: &mut WaveBank, drive: f32, tone: f32) -> [f32; TABLE_SIZE] {
    bank.rebuild_morph();
    let mut out = *bank.morph_table();
    apply_drive(&mut out, drive);
    apply_tone(&mut out, tone);
    normalize_peak(&mut out);
    out
}

/// Write mono 16-bit PCM WAV (single cycle), same format as Python `write_wavetable_wav`.
pub fn write_wavetable_wav(path: &Path, table: &[f32], sample_rate: u32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let samples: Vec<f32> = if table.len() == TABLE_SIZE {
        table.to_vec()
    } else if table.is_empty() {
        vec![0.0; TABLE_SIZE]
    } else {
        // Linear resample to TABLE_SIZE (bake path normally passes a full cycle).
        let n = table.len();
        (0..TABLE_SIZE)
            .map(|i| {
                let pos = (i as f64 / TABLE_SIZE as f64) * n as f64;
                let i0 = pos.floor() as usize % n;
                let i1 = (i0 + 1) % n;
                let frac = (pos - pos.floor()) as f32;
                table[i0] * (1.0 - frac) + table[i1] * frac
            })
            .collect()
    };
    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    let pcm: Vec<i16> = samples
        .iter()
        .map(|s| {
            let v = (*s / peak * 32767.0).clamp(-32768.0, 32767.0);
            v as i16
        })
        .collect();

    let data_bytes = (pcm.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes()); // PCM chunk size
    bytes.extend_from_slice(&1u16.to_le_bytes()); // audio format PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * 2;
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes()); // block align
    bytes.extend_from_slice(&16u16.to_le_bytes()); // bits
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for s in pcm {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let mut f = fs::File::create(path).map_err(|e| e.to_string())?;
    f.write_all(&bytes).map_err(|e| e.to_string())?;
    Ok(())
}

/// Sidecar matching Python `write_voice_fx_sidecar` keys (delay/reverb from bus).
pub fn write_fx_sidecar(
    path: &Path,
    delay_mix: f32,
    reverb_mix: f32,
    flanger_mix: f32,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::json!({
        "version": 1,
        "fx_delay_time": 0.0,
        "fx_delay_fb": 0.0,
        "fx_delay_mix": delay_mix.clamp(0.0, 1.0),
        "fx_reverb_size": 0.0,
        "fx_reverb_mix": reverb_mix.clamp(0.0, 1.0),
        "fx_flanger_mix": flanger_mix.clamp(0.0, 1.0),
    });
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?),
    )
    .map_err(|e| e.to_string())
}

pub struct BakeResult {
    pub name: String,
    pub wav_path: PathBuf,
    pub fx_path: PathBuf,
    pub index: usize,
}

/// Bake current morph, write WAV + `.fx.json`, insert into bank, select as A=B.
pub fn save_as(
    bank: &mut WaveBank,
    wave_names: &[String],
    name_a: &str,
    name_b: &str,
    morph: f32,
    drive: f32,
    tone: f32,
    delay_mix: f32,
    reverb_mix: f32,
    flanger_mix: f32,
) -> Result<BakeResult, String> {
    let base = suggest_voice_name(name_a, name_b, morph);
    let name = unique_voice_name(&base, wave_names);
    let cycle = bake_cycle(bank, drive, tone);
    let dir = user_wavetables_dir();
    let wav_path = dir.join(format!("{name}.wav"));
    let fx_path = dir.join(format!("{name}.fx.json"));
    write_wavetable_wav(&wav_path, &cycle, SAMPLE_RATE)?;
    write_fx_sidecar(&fx_path, delay_mix, reverb_mix, flanger_mix)?;
    let index = bank.insert(&name, &cycle);
    bank.set_morph_pair(index, index);
    bank.set_morph(0.0);
    bank.rebuild_morph();
    Ok(BakeResult {
        name,
        wav_path,
        fx_path,
        index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_and_unique() {
        assert_eq!(sanitize_voice_name("Warm Pad!"), "warm_pad");
        let existing = vec!["sine".into(), "saw_square_50".into()];
        assert_eq!(
            unique_voice_name("saw_square_50", &existing),
            "saw_square_50_2"
        );
    }

    #[test]
    fn bake_writes_wav_and_sidecar() {
        let dir = std::env::temp_dir().join(format!("pidi-bake-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        std::env::set_var("JAMBOX_USER_WAVETABLES", &dir);

        let mut bank = WaveBank::with_builtins();
        bank.set_morph_pair(0, 2);
        bank.set_morph(0.4);
        let names = bank.names().to_vec();
        let result = save_as(
            &mut bank,
            &names,
            "sine",
            "saw",
            0.4,
            0.2,
            0.7,
            0.3,
            0.1,
            0.4,
        )
        .unwrap();
        assert!(result.wav_path.is_file());
        assert!(result.fx_path.is_file());
        let fx: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&result.fx_path).unwrap()).unwrap();
        assert!((fx["fx_delay_mix"].as_f64().unwrap() - 0.3).abs() < 1e-6);
        assert!((fx["fx_reverb_mix"].as_f64().unwrap() - 0.1).abs() < 1e-6);
        assert!((fx["fx_flanger_mix"].as_f64().unwrap() - 0.4).abs() < 1e-6);
        let meta = fs::metadata(&result.wav_path).unwrap();
        assert!(meta.len() > 44);

        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("JAMBOX_USER_WAVETABLES");
    }
}
