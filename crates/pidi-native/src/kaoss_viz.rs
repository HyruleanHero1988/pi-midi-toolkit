//! Kaoss pad visualization (parity with `apps/pidi/pidi/kaoss.py`).

use crate::model::{LED_COLS, LED_ROWS};

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let h = h.rem_euclid(1.0);
    let s = clamp01(s);
    let v = clamp01(v);
    if s <= 0.0 {
        let c = (v * 255.0).round() as u8;
        return (c, c, c);
    }
    let sector = h * 6.0;
    let i = sector.floor() as i32;
    let f = sector - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    (
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

pub fn pack_rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

pub fn hsv_color(h: f32, s: f32, v: f32) -> u32 {
    let (r, g, b) = hsv_to_rgb(h, s, v);
    pack_rgb(r, g, b)
}

pub fn program_hue(program_id: &str) -> f32 {
    match program_id {
        "lead" => 0.93,
        "morph" => 0.80,
        "vib" => 0.55,
        "level" => 0.12,
        "decay" => 0.08,
        "attack" => 0.18,
        "octave" => 0.45,
        "grit" => 0.02,
        "delay" => 0.62,
        "filter" => 0.72,
        "echo" => 0.58,
        "drive" => 0.04,
        "space" => 0.66,
        "reso" => 0.85,
        "wash" => 0.70,
        "crush" => 0.98,
        "sweep" => 0.50,
        _ => 0.93,
    }
}

pub fn glow_step(current: f32, target: f32, dt: f32) -> f32 {
    let cur = clamp01(current);
    let tgt = clamp01(target);
    let step = dt.clamp(0.0, 0.08);
    if tgt >= cur {
        if tgt == cur {
            return cur;
        }
        return clamp01(cur + step / 0.16);
    }
    clamp01(cur - step / 0.32)
}

pub fn glow_radii(span: f32, amp: f32) -> (f32, f32, f32) {
    let span = span.max(1.0);
    let amp = clamp01(amp);
    let scale = 0.82;
    let outer = span * 0.52 * scale * amp.powf(1.35);
    let mid = span * 0.28 * scale * amp.powf(1.12);
    let core = span * 0.11 * scale * amp;
    (outer, mid, core)
}

pub fn viz_pulse(t: f32, bpm: f32, gate_flash: f32) -> f32 {
    let period = 60.0 / bpm.clamp(40.0, 240.0);
    let phase = t.rem_euclid(period) / period;
    let beat = (1.0 - phase * 5.0).max(0.0);
    (beat * 0.45).max(gate_flash * 0.9)
}

/// One LED in CELLS mode (Tk `pad_led_hex`).
pub fn pad_led_rgb(
    col: usize,
    row: usize,
    t: f32,
    finger: Option<(f32, f32)>,
    trail: &[(f32, f32, f32)],
    ripples: &[(f32, f32, f32)],
    hold: bool,
    gate_flash: f32,
    hue_shift: f32,
) -> u32 {
    let cols = LED_COLS.max(2);
    let rows = LED_ROWS.max(2);
    let lx = col as f32 / (cols - 1) as f32;
    let ly = row as f32 / (rows - 1) as f32;
    let wave = 0.5 + 0.5 * (t * 1.6 + col as f32 * 0.45 + row as f32 * 0.38).sin();
    let mut hue = (lx * 0.70 + hue_shift + t * 0.035).rem_euclid(1.0);
    let mut sat = 0.82;
    let mut val = 0.045 + 0.09 * wave;
    if hold {
        val += 0.05;
    }
    if let Some((fx, fy)) = finger {
        let dist = ((lx - fx).hypot(ly - fy)).abs();
        let glow = (1.0 - dist / 0.40).max(0.0).powf(1.45);
        val = (val + glow * 0.92).min(1.0);
        sat = (0.55 + glow * 0.45).min(1.0);
        hue = (hue * (1.0 - glow * 0.55) + (fx * 0.70 + hue_shift) * glow).rem_euclid(1.0);
    }
    for &(tx, ty, age) in trail {
        let dist = ((lx - tx).hypot(ly - ty)).abs();
        let spark = (1.0 - dist / 0.22).max(0.0).powf(1.8) * clamp01(age) * 0.55;
        val = (val + spark).min(1.0);
    }
    for &(rx, ry, age) in ripples {
        let age = clamp01(age);
        let radius = 0.08 + age * 0.72;
        let dist = ((lx - rx).hypot(ly - ry)).abs();
        let ring = (1.0 - (dist - radius).abs() / 0.10).max(0.0);
        val = (val + ring * (1.0 - age) * 0.65).min(1.0);
    }
    val = (val + clamp01(gate_flash) * 0.20).min(1.0);
    hsv_color(hue, sat, val)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_primary_colors() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), (255, 0, 0));
        assert_eq!(hsv_to_rgb(1.0 / 3.0, 1.0, 1.0), (0, 255, 0));
        assert_eq!(hsv_to_rgb(2.0 / 3.0, 1.0, 1.0), (0, 0, 255));
        assert_eq!(hsv_to_rgb(0.0, 0.0, 0.0), (0, 0, 0));
    }

    #[test]
    fn glow_envelope_eases() {
        assert!(glow_step(0.0, 1.0, 0.05) < 0.45);
        assert!(glow_step(0.0, 1.0, 0.05) > 0.2);
        assert!(glow_step(1.0, 0.0, 0.05) > 0.7);
        assert_eq!(glow_step(1.0, 1.0, 0.05), 1.0);
    }

    #[test]
    fn glow_radii_scale_with_amp() {
        let span = 400.0;
        let full = glow_radii(span, 1.0);
        let faded = glow_radii(span, 0.2);
        assert!(full.0 > faded.0);
        assert!(full.1 > faded.1);
        assert!(full.2 > faded.2);
    }

    #[test]
    fn cells_are_colorful_not_monotone() {
        let a = pad_led_rgb(0, 0, 0.0, None, &[], &[], false, 0.0, 0.93);
        let b = pad_led_rgb(11, 0, 0.0, None, &[], &[], false, 0.0, 0.93);
        assert_ne!(a, b, "columns should differ in hue");
        let r_a = (a >> 16) & 0xff;
        let g_a = (a >> 8) & 0xff;
        let b_a = a & 0xff;
        assert!(
            r_a.max(g_a).max(b_a) > r_a.min(g_a).min(b_a) + 5,
            "idle cells should be chromatic, not gray"
        );
    }
}
