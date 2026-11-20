//! Offline A/B WAV renders for the bus phaser (white noise + saw).
//! Usage: cargo run -p jambox-core --example phaser_render -- /opt/cursor/artifacts

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
    let saw_inc = 110.0f32 / SR;
    let mut rng = 0xC0FF_EEu32;
    for s in buf.iter_mut() {
        phase += saw_inc;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        let saw = phase * 2.0 - 1.0;
        rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (rng as f32 / u32::MAX as f32) * 2.0 - 1.0;
        *s = 0.55 * saw + 0.45 * noise;
    }
    buf
}

fn legacy_ap_only_mix(buf: &mut [f32], mix: f32) {
    let mut phaser_z = [0.0f32; 6];
    let mut phaser_phase = 0.0f32;
    let mut phaser_fb_z = 0.0f32;
    let rate_hz = 0.10 + 0.35 * 4.0;
    let depth = 0.70f32;
    let fb = 0.35f32;
    let phase_inc = rate_hz / SR;
    let two_pi = std::f32::consts::TAU;
    for s in buf.iter_mut() {
        let dry = 1.0 - mix;
        phaser_phase += phase_inc;
        if phaser_phase >= 1.0 {
            phaser_phase -= 1.0;
        }
        let lfo = 0.5 + 0.5 * (phaser_phase * two_pi).sin();
        let coeff = (0.15 + 0.70 * depth * lfo).clamp(0.05, 0.95);
        let mut x = *s + phaser_fb_z * fb;
        for z in phaser_z.iter_mut() {
            let y = coeff * x + *z;
            *z = x - coeff * y;
            x = y;
        }
        phaser_fb_z = x;
        *s = *s * dry + x * mix;
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
    legacy_ap_only_mix(&mut before, 1.0);

    let mut after = dry.clone();
    let mut fx = FxUnit::new(SR);
    fx.set_params(FxParams {
        phaser_mix: 1.0,
        phaser_rate: 0.35,
        phaser_depth: 0.85,
        phaser_fb: 0.55,
        ..FxParams::default()
    });
    fx.process(&mut after);

    write_wav(&dir.join("phaser_before_legacy_mix.wav"), &before);
    write_wav(&dir.join("phaser_after_classic_mix.wav"), &after);
    write_wav(&dir.join("phaser_dry_saw_noise.wav"), &dry);
    println!("wrote renders under {}", dir.display());
}