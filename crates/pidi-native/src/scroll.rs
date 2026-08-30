//! Finger-drag scrolling (Tk `TOUCH_SCROLL_THRESH_PX` parity).

use crate::layout::Rect;

/// Pixels of vertical movement before a press becomes a scroll instead of a tap.
pub const TOUCH_SCROLL_THRESH_PX: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollKind {
    SynthMorphPick,
    KaossPicker,
    KaossSettings,
    SongList,
    Log,
    Home,
}

/// Fixed-row grid inside a viewport (voice grids, kaoss program picker, …).
#[derive(Debug, Clone, Copy)]
pub struct GridScroll {
    pub cols: usize,
    pub cell_h: i32,
    pub item_count: usize,
    pub viewport: Rect,
}

impl GridScroll {
    pub fn rows(&self) -> usize {
        self.item_count.div_ceil(self.cols.max(1))
    }

    pub fn content_h(&self) -> i32 {
        self.rows() as i32 * self.cell_h
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
            y: self.viewport.y + row * self.cell_h - scroll_y + 4,
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
        let first_row = (scroll_y / self.cell_h.max(1)).max(0) as usize;
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
}
