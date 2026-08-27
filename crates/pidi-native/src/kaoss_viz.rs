//! Kaoss pad visualization (parity with `apps/pidi/pidi/kaoss.py`).

use crate::model::{LED_COLS, LED_ROWS};

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Pad visualizer shape: LED grid vs soft radial bloom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KaossVizStyle {
    /// 12×7 LED field.
    #[default]
    Cells,
    /// Free-form soft radial bloom.
    Glow,
}

impl KaossVizStyle {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cells => "CELLS",
            Self::Glow => "GLOW",
        }
    }

    pub fn is_cells(self) -> bool {
        matches!(self, Self::Cells)
    }

    pub fn is_glow(self) -> bool {
        matches!(self, Self::Glow)
    }

    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "glow" => Self::Glow,
            // Legacy "rainbow" / "mono" / "cells" all map to the LED grid;
            // solid vs rainbow is owned by [`pad_color_index`].
            "mono" | "static" | "cells" | "rainbow" | _ => Self::Cells,
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            Self::Cells => "cells",
            Self::Glow => "glow",
        }
    }
}

/// Pad color choices shared by CELLS and GLOW.
/// Index 0 = RAINBOW; the rest are solid hues (legacy mono palette).
pub const PAD_COLORS: &[(&str, Option<(f32, f32)>)] = &[
    ("RAINBOW", None),
    ("PINK", Some((0.93, 0.88))),
    ("PURPLE", Some((0.78, 0.85))),
    ("CYAN", Some((0.52, 0.82))),
    ("GREEN", Some((0.33, 0.78))),
    ("AMBER", Some((0.10, 0.90))),
    ("RED", Some((0.00, 0.90))),
    ("WHITE", Some((0.00, 0.00))),
];

/// Legacy mono palette (solids only) — kept for older session indices that
/// stored 0=PINK before RAINBOW was prepended. Prefer [`PAD_COLORS`].
pub const MONO_PALETTE: &[(&str, f32, f32)] = &[
    ("PINK", 0.93, 0.88),
    ("PURPLE", 0.78, 0.85),
    ("CYAN", 0.52, 0.82),
    ("GREEN", 0.33, 0.78),
    ("AMBER", 0.10, 0.90),
    ("RED", 0.00, 0.90),
    ("WHITE", 0.00, 0.00),
];

pub fn pad_color_count() -> usize {
    PAD_COLORS.len()
}

pub fn pad_color_label(index: usize) -> &'static str {
    PAD_COLORS[index % PAD_COLORS.len()].0
}

pub fn pad_color_is_rainbow(index: usize) -> bool {
    PAD_COLORS[index % PAD_COLORS.len()].1.is_none()
}

/// Solid hue/sat, or `None` for rainbow.
pub fn pad_color_hs(index: usize) -> Option<(f32, f32)> {
    PAD_COLORS[index % PAD_COLORS.len()].1
}

/// Migrate a pre-rainbow mono index (0=PINK…) into [`PAD_COLORS`] (0=RAINBOW).
pub fn migrate_legacy_mono_color(index: usize, style_was_mono: bool) -> usize {
    if style_was_mono {
        // Old mono index 0..6 → PAD_COLORS 1..7
        (index % MONO_PALETTE.len()) + 1
    } else {
        // Old rainbow / cells / glow with unused mono index → RAINBOW
        0
    }
}

pub fn mono_color_label(index: usize) -> &'static str {
    pad_color_label(index)
}

pub fn mono_color_hs(index: usize) -> (f32, f32) {
    pad_color_hs(index).unwrap_or((0.93, 0.88))
}

pub fn cycle_mono_color(index: usize) -> usize {
    (index + 1) % PAD_COLORS.len()
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
        "bend" => 0.42,
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

/// CELLS excitation envelope: snappy attack, slower release than GLOW.
pub fn cell_step(current: f32, target: f32, dt: f32) -> f32 {
    let cur = clamp01(current);
    let tgt = clamp01(target);
    let step = dt.clamp(0.0, 0.08);
    if tgt >= cur {
        if tgt == cur {
            return cur;
        }
        // ~70 ms to full
        return clamp01(cur + step / 0.07);
    }
    // ~420 ms to dark
    clamp01(cur - step / 0.42)
}

/// Instantaneous touch/trail energy for one LED (0..1), before envelope.
pub fn cell_excit_target(
    col: usize,
    row: usize,
    finger: Option<(f32, f32)>,
    trail: &[(f32, f32, f32)],
) -> f32 {
    let cols = LED_COLS.max(2);
    let rows = LED_ROWS.max(2);
    let lx = col as f32 / (cols - 1) as f32;
    let ly = row as f32 / (rows - 1) as f32;
    let mut e = 0.0_f32;
    if let Some((fx, fy)) = finger {
        let dist = ((lx - fx).hypot(ly - fy)).abs();
        let glow = (1.0 - dist / 0.40).max(0.0).powf(1.45);
        e = (e + glow).min(1.0);
    }
    for &(tx, ty, age) in trail {
        let dist = ((lx - tx).hypot(ly - ty)).abs();
        let spark = (1.0 - dist / 0.22).max(0.0).powf(1.8) * clamp01(age) * 0.55;
        e = (e + spark).min(1.0);
    }
    e
}

/// Outer radius of the soft radial bloom (span = min(pad w,h)).
pub fn glow_outer_radius(span: f32, amp: f32) -> f32 {
    let span = span.max(1.0);
    let amp = clamp01(amp);
    span * 0.55 * amp.powf(1.15)
}

/// Soft HSV for a radial sample: `fall` 0 = edge, 1 = core.
pub fn glow_sample(hue: f32, amp: f32, fall: f32, pulse: f32) -> u32 {
    let fall = clamp01(fall);
    let amp = clamp01(amp);
    // Ease so the bright core is small and the halo fades gently.
    let soft = fall.powf(1.55);
    let sat = (0.92 - 0.78 * soft.powf(1.1)).clamp(0.12, 0.95);
    let val = ((0.08 + 0.90 * soft) * (0.35 + 0.65 * amp) + pulse * 0.06 * soft).min(1.0);
    hsv_color(hue, sat, val)
}

/// Hue for one glow ring. Rainbow mode spreads the spectrum across concentric shells.
pub fn glow_ring_hue(pad_color: usize, fall: f32, t: f32) -> f32 {
    if pad_color_is_rainbow(pad_color) {
        // Outer rings (fall≈0) → red; toward core → cyan/blue, slow drift.
        ((1.0 - clamp01(fall)) * 0.85 + t * 0.025).rem_euclid(1.0)
    } else {
        pad_color_hs(pad_color).map(|(h, _)| h).unwrap_or(0.93)
    }
}

/// Saturation for glow wash / solid rings (rainbow keeps full chroma).
pub fn glow_ring_sat(pad_color: usize) -> f32 {
    if pad_color_is_rainbow(pad_color) {
        0.92
    } else {
        pad_color_hs(pad_color)
            .map(|(_, s)| s.max(0.35))
            .unwrap_or(0.88)
    }
}

/// Resolve session style wire + stored color index into (style, pad color).
pub fn load_viz_from_session(style_name: &str, mono_color: usize) -> (KaossVizStyle, usize) {
    let style = KaossVizStyle::from_name(style_name);
    let lower = style_name.to_ascii_lowercase();
    let color = match lower.as_str() {
        "mono" | "static" => migrate_legacy_mono_color(mono_color, true),
        "cells" | "glow" => mono_color % PAD_COLORS.len(),
        // Legacy rainbow (and unknown): rainbow color, cells unless glow.
        _ => migrate_legacy_mono_color(mono_color, false),
    };
    (style, color)
}

/// Lag shells for GLOW: index 0 = outermost (slowest), last = core (fastest).
pub const GLOW_LAG_COUNT: usize = 10;

/// Time constant (seconds) for one lag shell — outer rings take longer to catch up.
pub fn glow_lag_tau(layer: usize) -> f32 {
    let n = (GLOW_LAG_COUNT - 1).max(1) as f32;
    let u = (layer.min(GLOW_LAG_COUNT - 1) as f32) / n; // 0 outer → 1 core
    0.030 + (1.0 - u) * 0.22
}

/// Exponential ease of one shell toward the finger.
pub fn glow_lag_step(current: (f32, f32), target: (f32, f32), dt: f32, tau: f32) -> (f32, f32) {
    let a = 1.0 - (-dt / tau.max(0.001)).exp();
    let a = a.clamp(0.0, 1.0);
    (
        current.0 + (target.0 - current.0) * a,
        current.1 + (target.1 - current.1) * a,
    )
}

/// Interpolate a draw-ring center from lag shells.
/// `fall` 0 = outer edge (most lag), 1 = core (least lag).
pub fn glow_lag_xy(shells: &[(f32, f32); GLOW_LAG_COUNT], fall: f32) -> (f32, f32) {
    let fall = clamp01(fall);
    let max_i = (GLOW_LAG_COUNT - 1) as f32;
    let t = fall * max_i;
    let i0 = t.floor() as usize;
    let i1 = (i0 + 1).min(GLOW_LAG_COUNT - 1);
    let f = t - i0 as f32;
    let (x0, y0) = shells[i0];
    let (x1, y1) = shells[i1];
    (x0 + (x1 - x0) * f, y0 + (y1 - y0) * f)
}

pub fn viz_pulse(t: f32, bpm: f32, gate_flash: f32) -> f32 {
    let period = 60.0 / bpm.clamp(40.0, 240.0);
    let phase = t.rem_euclid(period) / period;
    let beat = (1.0 - phase * 5.0).max(0.0);
    (beat * 0.45).max(gate_flash * 0.9)
}

/// One LED in rainbow CELLS mode (Tk colorful `pad_led_hex`).
/// `excit` is the smoothed touch envelope (0..1); idle shimmer stays instant.
pub fn pad_led_rgb(
    col: usize,
    row: usize,
    t: f32,
    finger: Option<(f32, f32)>,
    excit: f32,
    hold: bool,
    gate_flash: f32,
    hue_shift: f32,
) -> u32 {
    let (_h, _s, val) = pad_led_base(col, row, t, excit, hold, gate_flash);
    let cols = LED_COLS.max(2);
    let lx = col as f32 / (cols - 1) as f32;
    let mut hue = (lx * 0.70 + hue_shift + t * 0.035).rem_euclid(1.0);
    let glow = clamp01(excit);
    let sat = (0.55 + glow * 0.45).min(1.0);
    if let Some((fx, _fy)) = finger {
        hue = (hue * (1.0 - glow * 0.55) + (fx * 0.70 + hue_shift) * glow).rem_euclid(1.0);
    }
    hsv_color(hue, if glow < 0.02 { 0.82 } else { sat }, val)
}

/// Monochrome LED field — same motion response, fixed palette hue/sat.
pub fn pad_led_mono(
    col: usize,
    row: usize,
    t: f32,
    excit: f32,
    hold: bool,
    gate_flash: f32,
    hue: f32,
    sat: f32,
) -> u32 {
    let (_h, _s, mut val) = pad_led_base(col, row, t, excit, hold, gate_flash);
    // Extra proximity pop so mono reads clearly once the envelope is up.
    val = (val + clamp01(excit) * 0.35).min(1.0);
    hsv_color(hue, sat, val)
}

fn pad_led_base(
    col: usize,
    row: usize,
    t: f32,
    excit: f32,
    hold: bool,
    gate_flash: f32,
) -> (f32, f32, f32) {
    let wave = 0.5 + 0.5 * (t * 1.6 + col as f32 * 0.45 + row as f32 * 0.38).sin();
    let mut val = 0.045 + 0.09 * wave;
    if hold {
        val += 0.05;
    }
    val = (val + clamp01(excit) * 0.92).min(1.0);
    val = (val + clamp01(gate_flash) * 0.20).min(1.0);
    (0.0, 0.82, val)
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
    fn glow_outer_scales_with_amp() {
        let span = 400.0;
        assert!(glow_outer_radius(span, 1.0) > glow_outer_radius(span, 0.2));
        assert!(glow_outer_radius(span, 1.0) > 100.0);
    }

    #[test]
    fn glow_sample_brightens_toward_core() {
        let edge = glow_sample(0.93, 1.0, 0.0, 0.0);
        let core = glow_sample(0.93, 1.0, 1.0, 0.0);
        let edge_v = ((edge >> 16) & 0xff).max((edge >> 8) & 0xff).max(edge & 0xff);
        let core_v = ((core >> 16) & 0xff).max((core >> 8) & 0xff).max(core & 0xff);
        assert!(core_v > edge_v + 40, "core should be brighter than the halo edge");
    }

    #[test]
    fn glow_lag_outer_is_slower_than_core() {
        assert!(glow_lag_tau(0) > glow_lag_tau(GLOW_LAG_COUNT - 1));
        let start = (0.0, 0.0);
        let target = (1.0, 1.0);
        let outer = glow_lag_step(start, target, 0.016, glow_lag_tau(0));
        let core = glow_lag_step(start, target, 0.016, glow_lag_tau(GLOW_LAG_COUNT - 1));
        assert!(
            core.0 > outer.0,
            "core should close more of the gap per frame"
        );
    }

    #[test]
    fn glow_lag_xy_maps_fall_to_shells() {
        let mut shells = [(0.0, 0.0); GLOW_LAG_COUNT];
        shells[GLOW_LAG_COUNT - 1] = (1.0, 0.5);
        let core = glow_lag_xy(&shells, 1.0);
        let edge = glow_lag_xy(&shells, 0.0);
        assert!((core.0 - 1.0).abs() < 1e-4);
        assert!((edge.0 - 0.0).abs() < 1e-4);
    }

    #[test]
    fn cell_envelope_attacks_faster_than_release() {
        let up = cell_step(0.0, 1.0, 0.05);
        let down = cell_step(1.0, 0.0, 0.05);
        assert!(up > 0.55, "quick fade-in, got {up}");
        assert!(down > 0.85, "slower fade-out, got {down}");
        assert!(up > (1.0 - down), "attack should outpace release");
    }

    #[test]
    fn cells_are_colorful_not_monotone() {
        let a = pad_led_rgb(0, 0, 0.0, None, 0.0, false, 0.0, 0.93);
        let b = pad_led_rgb(11, 0, 0.0, None, 0.0, false, 0.0, 0.93);
        assert_ne!(a, b, "columns should differ in hue");
        let r_a = (a >> 16) & 0xff;
        let g_a = (a >> 8) & 0xff;
        let b_a = a & 0xff;
        assert!(
            r_a.max(g_a).max(b_a) > r_a.min(g_a).min(b_a) + 5,
            "idle cells should be chromatic, not gray"
        );
    }

    #[test]
    fn mono_stays_single_hue_family() {
        let a = pad_led_mono(0, 0, 0.0, 0.0, false, 0.0, 0.93, 0.88);
        let b = pad_led_mono(11, 6, 0.5, 1.0, false, 0.0, 0.93, 0.88);
        let r_a = (a >> 16) & 0xff;
        let g_a = (a >> 8) & 0xff;
        let r_b = (b >> 16) & 0xff;
        assert!(r_a >= g_a, "mono pink idle should be reddish");
        assert!(r_b > 20, "touched mono cell should light up");
    }

    #[test]
    fn legacy_mono_migrates_past_rainbow_slot() {
        assert_eq!(migrate_legacy_mono_color(0, true), 1); // PINK
        assert_eq!(migrate_legacy_mono_color(0, false), 0); // RAINBOW
        let (style, color) = load_viz_from_session("mono", 0);
        assert_eq!(style, KaossVizStyle::Cells);
        assert_eq!(color, 1);
        let (style, color) = load_viz_from_session("rainbow", 0);
        assert_eq!(style, KaossVizStyle::Cells);
        assert_eq!(color, 0);
        let (style, color) = load_viz_from_session("glow", 3);
        assert_eq!(style, KaossVizStyle::Glow);
        assert_eq!(color, 3);
    }

    #[test]
    fn rainbow_glow_rings_differ_in_hue() {
        let outer = glow_ring_hue(0, 0.0, 0.0);
        let mid = glow_ring_hue(0, 0.5, 0.0);
        let core = glow_ring_hue(0, 1.0, 0.0);
        assert!((outer - mid).abs() > 0.1);
        assert!((mid - core).abs() > 0.1);
        // Solid color: same hue regardless of fall.
        assert!((glow_ring_hue(1, 0.0, 0.0) - glow_ring_hue(1, 1.0, 0.0)).abs() < 1e-4);
    }
}
