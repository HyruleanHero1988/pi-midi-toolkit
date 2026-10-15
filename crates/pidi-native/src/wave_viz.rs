//! Three-signal wave view: left, right, and the low end of the mix.
//!
//! Each signal has its own orientation, baseline, color, and thickness.
//! The first layout is a blue horizontal wave, a green vertical wave, and a
//! red circle in the center that the sub spikes out from.

use serde::{Deserialize, Serialize};

use crate::layout::{Hit, Rect};

pub const SUB_HZ_MIN: f32 = 20.0;
pub const SUB_HZ_MAX: f32 = 200.0;
pub const THICK_MIN: f32 = 1.0;
pub const THICK_MAX: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WaveOrient {
    #[default]
    Horizontal,
    Vertical,
    Circular,
}

impl WaveOrient {
    pub fn cycle(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Circular,
            Self::Circular => Self::Horizontal,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Horizontal => "HORIZ",
            Self::Vertical => "VERT",
            Self::Circular => "CIRC",
        }
    }

    pub fn offset_label(self) -> &'static str {
        match self {
            Self::Circular => "RAD",
            _ => "BASE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WaveSignal {
    #[serde(default)]
    pub orient: WaveOrient,
    #[serde(default = "default_offset")]
    pub offset: f32,
    #[serde(default = "default_rgb")]
    pub rgb: [u8; 3],
    #[serde(default = "default_thickness")]
    pub thickness: f32,
}

fn default_offset() -> f32 {
    0.5
}

fn default_rgb() -> [u8; 3] {
    [255, 255, 255]
}

fn default_thickness() -> f32 {
    3.0
}

impl WaveSignal {
    pub fn color(self) -> u32 {
        ((self.rgb[0] as u32) << 16) | ((self.rgb[1] as u32) << 8) | self.rgb[2] as u32
    }
}

/// Blue horizontal, green vertical, red center circle.
pub fn default_signals() -> [WaveSignal; 3] {
    [
        WaveSignal {
            orient: WaveOrient::Horizontal,
            offset: 0.5,
            rgb: [40, 130, 255],
            thickness: 3.0,
        },
        WaveSignal {
            orient: WaveOrient::Vertical,
            offset: 0.5,
            rgb: [40, 210, 70],
            thickness: 3.0,
        },
        WaveSignal {
            orient: WaveOrient::Circular,
            offset: 0.62,
            rgb: [230, 40, 40],
            thickness: 3.0,
        },
    ]
}

pub fn default_sub_hz() -> f32 {
    80.0
}

pub fn cutoff_from_unit(unit: f32) -> f32 {
    SUB_HZ_MIN + unit.clamp(0.0, 1.0) * (SUB_HZ_MAX - SUB_HZ_MIN)
}

pub fn cutoff_unit(hz: f32) -> f32 {
    ((hz.clamp(SUB_HZ_MIN, SUB_HZ_MAX) - SUB_HZ_MIN) / (SUB_HZ_MAX - SUB_HZ_MIN)).clamp(0.0, 1.0)
}

pub fn thick_from_unit(unit: f32) -> f32 {
    THICK_MIN + unit.clamp(0.0, 1.0) * (THICK_MAX - THICK_MIN)
}

pub fn thick_unit(px: f32) -> f32 {
    ((px.clamp(THICK_MIN, THICK_MAX) - THICK_MIN) / (THICK_MAX - THICK_MIN)).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy)]
pub struct WaveCol {
    pub orient: Rect,
    pub rows: [Rect; 5],
}

#[derive(Debug, Clone, Copy)]
pub struct WaveChrome {
    pub setup: Rect,
    pub done: Rect,
    pub history: Rect,
    pub bypass: Rect,
    pub cols: [WaveCol; 3],
    pub cutoff: Rect,
}

pub fn chrome(content: Rect) -> WaveChrome {
    let setup = Rect {
        x: content.x + content.w - 112,
        y: content.y + 8,
        w: 100,
        h: 40,
    };
    let done = Rect {
        x: content.x + content.w - 120,
        y: content.y + content.h - 48,
        w: 108,
        h: 40,
    };
    let bypass = Rect {
        x: done.x - 232,
        y: done.y,
        w: 100,
        h: 40,
    };
    let history = Rect {
        x: done.x - 116,
        y: done.y,
        w: 100,
        h: 40,
    };
    let cutoff = Rect {
        x: content.x + 12,
        y: done.y,
        w: (bypass.x - content.x - 20).max(40),
        h: 40,
    };
    let top = content.y + 8;
    let bottom = cutoff.y - 8;
    let gap = 8;
    let col_w = (content.w - 24 - gap * 2) / 3;
    let mut cols = [WaveCol {
        orient: Rect {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        },
        rows: [Rect {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        }; 5],
    }; 3];
    for i in 0..3 {
        let x = content.x + 12 + i as i32 * (col_w + gap);
        let orient = Rect {
            x,
            y: top + 22,
            w: col_w,
            h: 36,
        };
        let row_top = orient.y + orient.h + 6;
        let row_h = ((bottom - row_top) / 5).clamp(28, 44);
        let mut rows = [Rect {
            x,
            y: 0,
            w: col_w,
            h: row_h,
        }; 5];
        for (n, row) in rows.iter_mut().enumerate() {
            row.y = row_top + n as i32 * row_h;
        }
        cols[i] = WaveCol { orient, rows };
    }
    WaveChrome {
        setup,
        done,
        cols,
        history,
        bypass,
        cutoff,
    }
}

/// The draggable part of a slider row, to the right of its label.
pub fn track(row: Rect) -> Rect {
    Rect {
        x: row.x + 56,
        y: row.y + 6,
        w: (row.w - 64).max(20),
        h: (row.h - 12).max(8),
    }
}

pub fn unit_at(row: Rect, px: i32) -> f32 {
    let t = track(row);
    ((px - t.x) as f32 / t.w.max(1) as f32).clamp(0.0, 1.0)
}

pub fn hit(content: Rect, px: i32, py: i32, setup: bool) -> Hit {
    let chrome = chrome(content);
    if setup {
        if chrome.done.contains(px, py) {
            return Hit::WaveVizDone;
        }
        if chrome.history.contains(px, py) {
            return Hit::WaveVizHistory;
        }
        if chrome.bypass.contains(px, py) {
            return Hit::WaveVizBypass;
        }
        if chrome.cutoff.contains(px, py) {
            return Hit::WaveVizSlider {
                signal: 3,
                param: 0,
            };
        }
        for (i, col) in chrome.cols.iter().enumerate() {
            if col.orient.contains(px, py) {
                return Hit::WaveVizOrient(i);
            }
            for (param, row) in col.rows.iter().enumerate() {
                if row.contains(px, py) {
                    return Hit::WaveVizSlider {
                        signal: i,
                        param: param as u8,
                    };
                }
            }
        }
        return Hit::None;
    }
    if chrome.setup.contains(px, py) {
        Hit::WaveVizSetup
    } else {
        Hit::None
    }
}

/// Screen points for one signal. Circular traces repeat the first point so
/// the ring closes. A silent signal still draws its baseline.
pub fn trace(signal: WaveSignal, samples: &[i16], area: Rect) -> Vec<(f32, f32)> {
    let n = samples.len().max(2);
    let mut pts = Vec::with_capacity(n + 1);
    let offset = signal.offset.clamp(0.0, 1.0);
    match signal.orient {
        WaveOrient::Horizontal => {
            let margin = 10.0;
            let span = (area.w as f32 - margin * 2.0).max(1.0);
            let base = area.y as f32 + margin + (1.0 - offset) * (area.h as f32 - margin * 2.0);
            let amp = area.h as f32 * 0.22;
            for i in 0..n {
                let x = area.x as f32 + margin + span * i as f32 / (n - 1) as f32;
                let s = sample_at(samples, i, n);
                pts.push((x, base - s * amp));
            }
        }
        WaveOrient::Vertical => {
            let margin = 10.0;
            let span = (area.h as f32 - margin * 2.0).max(1.0);
            let base = area.x as f32 + margin + offset * (area.w as f32 - margin * 2.0);
            let amp = area.w as f32 * 0.16;
            for i in 0..n {
                let y = area.y as f32 + margin + span * i as f32 / (n - 1) as f32;
                let s = sample_at(samples, i, n);
                pts.push((base + s * amp, y));
            }
        }
        WaveOrient::Circular => {
            let cx = area.x as f32 + area.w as f32 * 0.5;
            let cy = area.y as f32 + area.h as f32 * 0.5;
            let max_r = (area.w.min(area.h) as f32) * 0.46;
            let spike = max_r * 0.28;
            let radius = 12.0 + offset * (max_r - spike - 12.0).max(16.0);
            for i in 0..n {
                let ang =
                    (i as f32 / n as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                let s = sample_at(samples, i, n);
                let r = (radius + s * spike).max(0.0);
                pts.push((cx + r * ang.cos(), cy + r * ang.sin()));
            }
            if let Some(first) = pts.first().copied() {
                pts.push(first);
            }
        }
    }
    pts
}

fn sample_at(samples: &[i16], i: usize, n: usize) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let idx = if n <= 1 {
        0
    } else {
        i * (samples.len() - 1) / (n - 1)
    };
    (samples[idx.min(samples.len() - 1)] as f32 / 32767.0).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect {
            x: 0,
            y: 76,
            w: 800,
            h: 404,
        }
    }

    #[test]
    fn first_layout_matches_the_card() {
        let signals = default_signals();
        assert_eq!(signals[0].orient, WaveOrient::Horizontal);
        assert!(signals[0].rgb[2] > signals[0].rgb[0] && signals[0].rgb[2] > signals[0].rgb[1]);
        assert_eq!(signals[1].orient, WaveOrient::Vertical);
        assert!(signals[1].rgb[1] > signals[1].rgb[0] && signals[1].rgb[1] > signals[1].rgb[2]);
        assert_eq!(signals[2].orient, WaveOrient::Circular);
        assert!(signals[2].rgb[0] > signals[2].rgb[1] && signals[2].rgb[0] > signals[2].rgb[2]);
        assert!((default_sub_hz() - 80.0).abs() < 0.1);
    }

    #[test]
    fn same_orientation_can_use_different_baselines() {
        let mut a = default_signals()[0];
        let mut b = a;
        a.offset = 0.2;
        b.offset = 0.8;
        let samples = [0i16; 32];
        let ta = trace(a, &samples, area());
        let tb = trace(b, &samples, area());
        let ya = ta[0].1;
        let yb = tb[0].1;
        assert!((ya - yb).abs() > 40.0, "baselines collapsed: {ya} vs {yb}");
    }

    #[test]
    fn circle_baseline_grows_with_radius_and_spikes_outward() {
        let mut signal = default_signals()[2];
        signal.offset = 0.4;
        let quiet = trace(signal, &[0i16; 64], area());
        signal.offset = 0.9;
        let wider = trace(signal, &[0i16; 64], area());
        let rad = |pts: &[(f32, f32)]| {
            let (cx, cy) = (400.0, 76.0 + 202.0);
            let (x, y) = pts[0];
            ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()
        };
        assert!(rad(&wider) > rad(&quiet) + 20.0);
        let mut hot = default_signals()[2];
        hot.offset = 0.4;
        let spiked = trace(hot, &[32767i16; 64], area());
        assert!(rad(&spiked) > rad(&quiet) + 10.0);
    }

    #[test]
    fn config_hits_each_signal() {
        let content = area();
        assert!(matches!(
            hit(content, content.x + content.w - 40, content.y + 20, false),
            Hit::WaveVizSetup
        ));
        let chrome = chrome(content);
        assert!(matches!(
            hit(
                content,
                chrome.cols[1].orient.x + 4,
                chrome.cols[1].orient.y + 4,
                true
            ),
            Hit::WaveVizOrient(1)
        ));
        let row = chrome.cols[2].rows[2];
        assert!(matches!(
            hit(content, row.x + 8, row.y + 8, true),
            Hit::WaveVizSlider {
                signal: 2,
                param: 2
            }
        ));
    }
}
