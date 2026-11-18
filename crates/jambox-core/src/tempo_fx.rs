//! BPM-synced pad FX: snap delay time and mod rates to a beat grid.
//!
//! # Division mapping (pad axis 0..1)
//!
//! Same four bands as the punch RPT/SLICE grid — bottom of the pad is the
//! longest / slowest division, top is the shortest / fastest:
//!
//! | Pad band (unit) | Label | Beats |
//! |-----------------|-------|-------|
//! | 0.00 .. 0.25    | 1/4   | 1.0   |
//! | 0.25 .. 0.50    | 1/8   | 0.5   |
//! | 0.50 .. 0.75    | 1/16  | 0.25  |
//! | 0.75 .. 1.00    | 1/32  | 0.125 |
//!
//! - **Delay** (`delay_time`): echo gap = `beats * 60 / bpm`, then converted
//!   into the FxUnit 0..1 delay knob (see [`crate::fx::DELAY_TIME_SEC_MIN`]).
//! - **Mod rate** (`tone_lfo` / `flanger_rate`): one LFO cycle per that many
//!   beats → `hz = bpm / (60 * beats)`.
//!
//! Kaoss programs that scrub these axes (ECHO X, DUB Y, WAH Y, FL RATE X)
//! should call the converters with the live transport BPM.

use crate::engine::{TONE_LFO_HZ_MAX, TONE_LFO_HZ_MIN};
use crate::fx::{
    DELAY_TIME_SEC_MIN, DELAY_TIME_SEC_SPAN, FLANGER_RATE_HZ_MIN, FLANGER_RATE_HZ_SPAN,
};

/// Beat lengths for each pad band (quarter-note units).
pub const TEMPO_FX_GRID_BEATS: [f32; 4] = [1.0, 0.5, 0.25, 0.125];
/// Boundaries between 1/4 → 1/8 → 1/16 → 1/32 on the pad axis.
pub const TEMPO_FX_GRID_TICKS: [f32; 3] = [0.25, 0.50, 0.75];
pub const TEMPO_FX_GRID_LABELS: [&str; 4] = ["1/4", "1/8", "1/16", "1/32"];

/// Which grid band `unit` (0..1) falls into.
pub fn tempo_fx_grid_index(unit: f32) -> usize {
    let t = unit.clamp(0.0, 1.0);
    TEMPO_FX_GRID_TICKS
        .iter()
        .filter(|tick| t >= **tick)
        .count()
        .min(TEMPO_FX_GRID_LABELS.len() - 1)
}

/// `"1/4"` … `"1/32"` for a pad axis position.
pub fn tempo_fx_grid_label(unit: f32) -> &'static str {
    TEMPO_FX_GRID_LABELS[tempo_fx_grid_index(unit)]
}

fn beats_for_unit(unit: f32) -> f32 {
    TEMPO_FX_GRID_BEATS[tempo_fx_grid_index(unit)]
}

fn beat_sec(bpm: f32) -> f32 {
    60.0 / bpm.max(1.0)
}

/// Delay gap in seconds for pad `unit` at `bpm`, clamped to the FxUnit range.
pub fn tempo_fx_delay_sec(unit: f32, bpm: f32) -> f32 {
    let sec = beats_for_unit(unit) * beat_sec(bpm);
    sec.clamp(DELAY_TIME_SEC_MIN, DELAY_TIME_SEC_MIN + DELAY_TIME_SEC_SPAN)
}

/// Pad axis → FxUnit `delay_time` (0..1) snapped to the tempo grid.
pub fn tempo_fx_delay_unit(unit: f32, bpm: f32) -> f32 {
    let sec = tempo_fx_delay_sec(unit, bpm);
    ((sec - DELAY_TIME_SEC_MIN) / DELAY_TIME_SEC_SPAN).clamp(0.0, 1.0)
}

/// LFO rate in Hz: one cycle per grid division at `bpm`.
pub fn tempo_fx_lfo_hz(unit: f32, bpm: f32) -> f32 {
    let beats = beats_for_unit(unit);
    bpm.max(1.0) / (60.0 * beats.max(1e-6))
}

/// Pad axis → tone-LFO synth unit (0..1) snapped to the tempo grid.
pub fn tempo_fx_tone_lfo_unit(unit: f32, bpm: f32) -> f32 {
    let hz = tempo_fx_lfo_hz(unit, bpm).clamp(TONE_LFO_HZ_MIN, TONE_LFO_HZ_MAX);
    // Inverse of `tone_lfo_hz_from_unit` (exponential 0.3..10 Hz).
    let span = (TONE_LFO_HZ_MAX / TONE_LFO_HZ_MIN).ln();
    ((hz / TONE_LFO_HZ_MIN).ln() / span).clamp(0.0, 1.0)
}

/// Pad axis → FxUnit `flanger_rate` (0..1) snapped to the tempo grid.
pub fn tempo_fx_flanger_rate_unit(unit: f32, bpm: f32) -> f32 {
    let hz = tempo_fx_lfo_hz(unit, bpm).clamp(
        FLANGER_RATE_HZ_MIN,
        FLANGER_RATE_HZ_MIN + FLANGER_RATE_HZ_SPAN,
    );
    ((hz - FLANGER_RATE_HZ_MIN) / FLANGER_RATE_HZ_SPAN).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_bands_match_punch_style_ticks() {
        assert_eq!(tempo_fx_grid_label(0.0), "1/4");
        assert_eq!(tempo_fx_grid_label(0.24), "1/4");
        assert_eq!(tempo_fx_grid_label(0.25), "1/8");
        assert_eq!(tempo_fx_grid_label(0.49), "1/8");
        assert_eq!(tempo_fx_grid_label(0.50), "1/16");
        assert_eq!(tempo_fx_grid_label(0.74), "1/16");
        assert_eq!(tempo_fx_grid_label(0.75), "1/32");
        assert_eq!(tempo_fx_grid_label(1.0), "1/32");
    }

    #[test]
    fn delay_at_120_bpm_is_exact_note_lengths() {
        // 120 BPM → 0.5 s/beat. 1/4=0.5, 1/8=0.25, 1/16=0.125, 1/32=0.0625.
        assert!((tempo_fx_delay_sec(0.0, 120.0) - 0.5).abs() < 1e-5);
        assert!((tempo_fx_delay_sec(0.3, 120.0) - 0.25).abs() < 1e-5);
        assert!((tempo_fx_delay_sec(0.6, 120.0) - 0.125).abs() < 1e-5);
        assert!((tempo_fx_delay_sec(0.9, 120.0) - 0.0625).abs() < 1e-5);

        let u = tempo_fx_delay_unit(0.0, 120.0);
        let sec = DELAY_TIME_SEC_MIN + u * DELAY_TIME_SEC_SPAN;
        assert!((sec - 0.5).abs() < 1e-4);
    }

    #[test]
    fn delay_clamps_when_bpm_is_too_slow_for_quarter() {
        // 40 BPM → 1.5 s/beat; FxUnit max is 0.75 s.
        let sec = tempo_fx_delay_sec(0.0, 40.0);
        assert!((sec - (DELAY_TIME_SEC_MIN + DELAY_TIME_SEC_SPAN)).abs() < 1e-5);
        assert!((tempo_fx_delay_unit(0.0, 40.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn lfo_hz_is_one_cycle_per_division() {
        // 120 BPM: 1/4 → 2 Hz, 1/8 → 4 Hz, 1/16 → 8 Hz, 1/32 → 16 Hz.
        assert!((tempo_fx_lfo_hz(0.0, 120.0) - 2.0).abs() < 1e-5);
        assert!((tempo_fx_lfo_hz(0.3, 120.0) - 4.0).abs() < 1e-5);
        assert!((tempo_fx_lfo_hz(0.6, 120.0) - 8.0).abs() < 1e-5);
        assert!((tempo_fx_lfo_hz(0.9, 120.0) - 16.0).abs() < 1e-5);
    }

    #[test]
    fn tone_lfo_unit_round_trips_through_engine_map() {
        use crate::engine::tone_lfo_hz_from_unit;
        for unit in [0.0f32, 0.3, 0.6, 0.9] {
            let snapped = tempo_fx_tone_lfo_unit(unit, 120.0);
            let hz = tone_lfo_hz_from_unit(snapped);
            let want = tempo_fx_lfo_hz(unit, 120.0).clamp(TONE_LFO_HZ_MIN, TONE_LFO_HZ_MAX);
            assert!(
                (hz - want).abs() < 1e-3,
                "unit={unit}: got {hz} want {want}"
            );
        }
    }

    #[test]
    fn flanger_rate_unit_stays_in_engine_range() {
        let u = tempo_fx_flanger_rate_unit(0.3, 120.0); // 4 Hz
        let hz = FLANGER_RATE_HZ_MIN + u * FLANGER_RATE_HZ_SPAN;
        assert!((hz - 4.0).abs() < 1e-3);
        // 1/32 at 120 = 16 Hz → clamps to flanger max.
        assert!((tempo_fx_flanger_rate_unit(1.0, 120.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn nearby_pad_positions_in_a_band_share_one_snap() {
        let a = tempo_fx_delay_unit(0.26, 100.0);
        let b = tempo_fx_delay_unit(0.49, 100.0);
        assert!((a - b).abs() < 1e-6);
        assert_eq!(tempo_fx_grid_label(0.26), tempo_fx_grid_label(0.49));
    }
}
