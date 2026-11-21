//! Offline A/B WAV renders for the bus tube DRIVE (saw + mild noise).
//! Usage: cargo run -p jambox-core --example drive_render -- /opt/cursor/artifacts

use jambox_core::{FxParams, FxUnit};
use std::{env, fs::File, io::Write, path::PathBuf};

const SR: f32 = 48_000.0;
const SECS: f32 = 3.0;

fn write_wav(path: &PathBuf, samples: &[f32]) {
    let mut f = File::create(path).expect("create wav");
    let data_bytes = (samples.len() * 2) as u32;
    let mut header = Vec::new();
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&(SR as u32).to_le_bytes());
    header.extend_from_slice(&(SR as u32).to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_bytes.to_le_bytes());
    f.write_all(&header).unwrap();
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        f.write_all(&v.to_le_bytes()).unwrap();
    }
}

fn saw_noise_buffer(frames: usize) -> Vec<f32> {
    let mut buf = vec![0.0f32; frames];
    let mut phase = 0.0f32;
    let saw_inc = 165.0f32 / SR;
    let mut rng = 0xD123_45u32;
    for s in buf.iter_mut() {
        phase += saw_inc;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        let saw = phase * 2.0 - 1.0;
        rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (rng as f32 / u32::MAX as f32) * 2.0 - 1.0;
        *s = 0.62 * saw + 0.12 * noise;
    }
    buf
}

#[inline]
fn soft_knee(x: f32, knee: f32, span: f32) -> f32 {
    if x <= knee {
        x
    } else {
        let d = x - knee;
        knee + d * span / (span + d)
    }
}

/// Pre-fix path: dry/wet crossfade + full inverse gain undid audible saturation.
fn legacy_drive_crossfade(buf: &mut [f32], drive: f32) {
    let drive_db = drive * 24.0;
    let gain = 0.35 * 10f32.powf(drive_db / 20.0);
    let inv_gain = 1.0 / gain.max(0.35);
    let pre = 0.18 * drive;
    let r = 0.995f32;
    let mut dc_x = 0.0f32;
    let mut dc_y = 0.0f32;
    for s in buf.iter_mut() {
        let dry = *s;
        let emphasized = dry + pre * (dry - dc_y);
        let hot = emphasized * gain;
        let clipped = if hot >= 0.0 {
            soft_knee(hot, 0.35, 0.56)
        } else {
            -soft_knee(-hot, 0.95, 1.20)
        };
        let shaped = clipped * inv_gain - pre * (clipped * inv_gain - dry) * 0.5;
        let y = shaped - dc_x + r * dc_y;
        dc_x = shaped;
        dc_y = y;
        *s = dry * (1.0 - drive) + y * drive;
    }
}

fn main() {
    let dir = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/cursor/artifacts"));
    std::fs::create_dir_all(&dir).ok();

    let frames = (SR * SECS) as usize;
    let dry = saw_noise_buffer(frames);

    let mut before = dry.clone();
    legacy_drive_crossfade(&mut before, 0.85);

    let mut moderate = dry.clone();
    let mut fx = FxUnit::new(SR);
    fx.set_params(FxParams {
        drive: 0.45,
        ..FxParams::default()
    });
    fx.process(&mut moderate);

    let mut after = dry.clone();
    let mut fx = FxUnit::new(SR);
    fx.set_params(FxParams {
        drive: 0.85,
        ..FxParams::default()
    });
    fx.process(&mut after);

    write_wav(&dir.join("drive_dry_saw_noise.wav"), &dry);
    write_wav(&dir.join("drive_before_legacy_crossfade.wav"), &before);
    write_wav(&dir.join("drive_after_moderate_045.wav"), &moderate);
    write_wav(&dir.join("drive_after_heavy_085.wav"), &after);
    println!("wrote renders under {}", dir.display());
}