//! Shared finger-drag scroll engine (smartphone-like tap vs scroll).
//!
//! Call sites start a [`ScrollGesture`] on press (including over buttons).
//! Movement past [`TOUCH_SCROLL_THRESH_PX`] steals the gesture into a scroll;
//! a release without that movement is a tap delivered to the child control.

use std::time::Instant;

use crate::layout::Rect;

/// Pixels of vertical movement before a press becomes a scroll instead of a tap.
pub const TOUCH_SCROLL_THRESH_PX: i32 = 10;

/// Minimum |velocity| (px/s) on lift to start a coast.
pub const SCROLL_FLING_MIN_VEL: f32 = 400.0;

/// Exponential decay rate for coasting (per second).
pub const SCROLL_COAST_DECAY: f32 = 6.0;

/// Stop coasting below this |velocity| (px/s).
pub const SCROLL_COAST_STOP: f32 = 40.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollKind {
    SynthMorphPick,
    KaossPicker,
    KaossSettings,
    SongList,
    Log,
    Home,
    FmPatches,
    /// Settings page option sheets (scale, Y bend, Y vel).
    SettingsPicker,
}

/// Finger-down → move → up lifecycle for one scroll viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollGesture {
    pub kind: ScrollKind,
    pub start_px: i32,
    pub start_py: i32,
    pub scroll_at_start: i32,
    pub dragging: bool,
    pub last_py: i32,
    pub last_move_at: Instant,
    /// Vertical velocity estimate (px/s). Positive = finger moved down → content up.
    pub vel_y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollPhase {
    /// Still within the tap threshold.
    Pending,
    /// Past threshold — this gesture owns scrolling.
    Dragging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollEnd {
    /// Released without crossing the threshold → deliver tap.
    Tap,
    /// Gesture became (or stayed) a scroll drag.
    Scrolled,
}

impl ScrollGesture {
    pub fn begin(kind: ScrollKind, px: i32, py: i32, scroll_at_start: i32) -> Self {
        Self {
            kind,
            start_px: px,
            start_py: py,
            scroll_at_start,
            dragging: false,
            last_py: py,
            last_move_at: Instant::now(),
            vel_y: 0.0,
        }
    }

    pub fn moved_enough(&self, py: i32) -> bool {
        (self.start_py - py).abs() >= TOUCH_SCROLL_THRESH_PX
    }

    pub fn phase_at(&self, py: i32) -> ScrollPhase {
        if self.dragging || self.moved_enough(py) {
            ScrollPhase::Dragging
        } else {
            ScrollPhase::Pending
        }
    }

    /// Content offset for the finger at `py` (drag up → larger scroll).
    pub fn offset_for(&self, py: i32) -> i32 {
        self.scroll_at_start + (self.start_py - py)
    }

    /// Update from a move sample; refreshes the fling velocity estimate.
    pub fn on_move(&mut self, py: i32) -> ScrollPhase {
        let now = Instant::now();
        let dt = now.duration_since(self.last_move_at).as_secs_f32();
        if dt > 0.000_1 && dt < 0.1 {
            let instant = (self.last_py - py) as f32 / dt;
            self.vel_y = self.vel_y * 0.65 + instant * 0.35;
        }
        self.last_py = py;
        self.last_move_at = now;
        let phase = self.phase_at(py);
        if phase == ScrollPhase::Dragging {
            self.dragging = true;
        }
        phase
    }

    pub fn on_up(&self, py: i32) -> ScrollEnd {
        if self.dragging || self.moved_enough(py) {
            ScrollEnd::Scrolled
        } else {
            ScrollEnd::Tap
        }
    }

    /// Fling velocity to coast with after a scrolled lift, if large enough.
    pub fn fling_velocity(&self) -> Option<f32> {
        if self.vel_y.abs() >= SCROLL_FLING_MIN_VEL {
            Some(self.vel_y)
        } else {
            None
        }
    }
}

/// Post-lift momentum for a scroll sheet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollCoast {
    pub kind: ScrollKind,
    pub velocity: f32,
}

impl ScrollCoast {
    pub fn from_fling(kind: ScrollKind, velocity: f32) -> Option<Self> {
        if velocity.abs() < SCROLL_FLING_MIN_VEL {
            None
        } else {
            Some(Self { kind, velocity })
        }
    }

    /// Advance coast by `dt` seconds. Returns the delta to add to the scroll offset,
    /// or `None` when coasting has stopped.
    pub fn tick(&mut self, dt: f32) -> Option<i32> {
        if self.velocity.abs() < SCROLL_COAST_STOP {
            return None;
        }
        let dt = dt.clamp(0.0, 0.05);
        let delta = (self.velocity * dt).round() as i32;
        self.velocity *= (-SCROLL_COAST_DECAY * dt).exp();
        if self.velocity.abs() < SCROLL_COAST_STOP {
            self.velocity = 0.0;
        }
        Some(delta)
    }
}

/// Fixed-row grid inside a viewport (voice grids, kaoss program picker, …).
#[derive(Debug, Clone, Copy)]
pub struct GridScroll {
    pub cols: usize,
    pub cell_h: i32,
    pub item_count: usize,
    pub viewport: Rect,
    /// Extra content above the first row (e.g. Home title/EDIT that scrolls away).
    pub top_inset: i32,
}

impl GridScroll {
    pub fn rows(&self) -> usize {
        self.item_count.div_ceil(self.cols.max(1))
    }

    pub fn content_h(&self) -> i32 {
        self.top_inset + self.rows() as i32 * self.cell_h
    }

    pub fn max_scroll(&self) -> i32 {
        (self.content_h() - self.viewport.h).max(0)
    }

    pub fn clamp_scroll(&self, y: i32) -> i32 {
        y.clamp(0, self.max_scroll())
    }

    pub fn cell_w(&self) -> i32 {
        self.viewport.w / self.cols.max(1) as i32
    }

    pub fn cell_rect(&self, index: usize, scroll_y: i32) -> Rect {
        let col = (index % self.cols) as i32;
        let row = (index / self.cols) as i32;
        let gw = self.cell_w();
        Rect {
            x: self.viewport.x + col * gw + 4,
            y: self.viewport.y + self.top_inset + row * self.cell_h - scroll_y + 4,
            w: gw - 8,
            h: self.cell_h - 8,
        }
    }

    pub fn index_at(&self, px: i32, py: i32, scroll_y: i32) -> Option<usize> {
        if !self.viewport.contains(px, py) {
            return None;
        }
        for i in 0..self.item_count {
            if self.cell_rect(i, scroll_y).contains(px, py) {
                return Some(i);
            }
        }
        None
    }

    pub fn visible_range(&self, scroll_y: i32) -> std::ops::Range<usize> {
        let shifted = (scroll_y - self.top_inset).max(0);
        let first_row = (shifted / self.cell_h.max(1)).max(0) as usize;
        let visible_rows = (self.viewport.h / self.cell_h.max(1) + 2) as usize;
        let start = first_row.saturating_mul(self.cols);
        let end = (start + visible_rows.saturating_mul(self.cols)).min(self.item_count);
        start..end
    }
}

/// Row-based list (songs, log lines).
#[derive(Debug, Clone, Copy)]
pub struct ListScroll {
    pub row_h: i32,
    pub item_count: usize,
    pub visible_rows: usize,
}

impl ListScroll {
    pub fn max_scroll(&self) -> usize {
        self.item_count.saturating_sub(self.visible_rows)
    }

    pub fn clamp_scroll(&self, index: usize) -> usize {
        index.min(self.max_scroll())
    }

    pub fn scroll_from_drag(start_index: usize, start_py: i32, py: i32, row_h: i32, max: usize) -> usize {
        let delta = (start_py - py) / row_h.max(1);
        (start_index as i32 + delta).clamp(0, max as i32) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_clamps_scroll() {
        let grid = GridScroll {
            cols: 4,
            cell_h: 50,
            item_count: 20,
            viewport: Rect {
                x: 0,
                y: 0,
                w: 400,
                h: 150,
            },
            top_inset: 0,
        };
        assert_eq!(grid.max_scroll(), 100);
        assert_eq!(grid.clamp_scroll(-10), 0);
        assert_eq!(grid.clamp_scroll(999), 100);
    }

    #[test]
    fn list_scroll_from_drag_uses_row_step() {
        let list = ListScroll {
            row_h: 56,
            item_count: 12,
            visible_rows: 5,
        };
        assert_eq!(
            ListScroll::scroll_from_drag(0, 200, 88, list.row_h, list.max_scroll()),
            2
        );
    }

    #[test]
    fn tap_vs_scroll_threshold() {
        let g = ScrollGesture::begin(ScrollKind::Home, 100, 200, 0);
        assert_eq!(g.phase_at(200), ScrollPhase::Pending);
        assert_eq!(g.phase_at(200 - (TOUCH_SCROLL_THRESH_PX - 1)), ScrollPhase::Pending);
        assert_eq!(g.on_up(200 - (TOUCH_SCROLL_THRESH_PX - 1)), ScrollEnd::Tap);

        let mut g = ScrollGesture::begin(ScrollKind::Home, 100, 200, 40);
        assert_eq!(g.on_move(200 - TOUCH_SCROLL_THRESH_PX), ScrollPhase::Dragging);
        assert!(g.dragging);
        assert_eq!(g.offset_for(200 - 30), 70);
        assert_eq!(g.on_up(200 - 30), ScrollEnd::Scrolled);
    }

    #[test]
    fn threshold_helper_matches_constant() {
        let g = ScrollGesture::begin(ScrollKind::KaossSettings, 0, 100, 0);
        assert!(!g.moved_enough(100 + TOUCH_SCROLL_THRESH_PX - 1));
        assert!(g.moved_enough(100 + TOUCH_SCROLL_THRESH_PX));
        assert!(g.moved_enough(100 - TOUCH_SCROLL_THRESH_PX));
    }

    #[test]
    fn coast_decays_and_stops() {
        let mut coast = ScrollCoast::from_fling(ScrollKind::Home, 800.0).expect("fling");
        let mut total = 0;
        for _ in 0..120 {
            match coast.tick(1.0 / 60.0) {
                Some(d) => total += d.abs(),
                None => break,
            }
        }
        assert!(total > 0);
        assert!(coast.velocity.abs() < SCROLL_COAST_STOP);
    }
}
