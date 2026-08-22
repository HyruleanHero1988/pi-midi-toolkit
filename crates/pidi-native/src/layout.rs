//! 800×480 hit-testing. Coordinates are pixels unless noted as 0..1 pad space.

pub const SCREEN_W: i32 = 800;
pub const SCREEN_H: i32 = 480;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Kaoss { x: f32, y: f32 },
    Drum { index: usize, note: u8 },
    Division(usize),
    None,
}

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    pub fn pad_xy(self, px: i32, py: i32) -> (f32, f32) {
        let x = (px - self.x) as f32 / self.w.max(1) as f32;
        let y = 1.0 - (py - self.y) as f32 / self.h.max(1) as f32;
        (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub hud: Rect,
    pub kaoss: Rect,
    pub drums: Rect,
    pub divisions: Rect,
    pub footer: Rect,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

impl Layout {
    pub fn new() -> Self {
        Self {
            hud: Rect {
                x: 0,
                y: 0,
                w: SCREEN_W,
                h: 40,
            },
            kaoss: Rect {
                x: 8,
                y: 48,
                w: 520,
                h: 368,
            },
            drums: Rect {
                x: 540,
                y: 48,
                w: 252,
                h: 252,
            },
            divisions: Rect {
                x: 540,
                y: 308,
                w: 252,
                h: 52,
            },
            footer: Rect {
                x: 0,
                y: 432,
                w: SCREEN_W,
                h: 48,
            },
        }
    }

    pub fn drum_cell(&self, index: usize) -> Rect {
        let col = (index % 4) as i32;
        let row_from_bottom = (index / 4) as i32;
        let row_from_top = 3 - row_from_bottom;
        let gw = self.drums.w / 4;
        let gh = self.drums.h / 4;
        Rect {
            x: self.drums.x + col * gw + 2,
            y: self.drums.y + row_from_top * gh + 2,
            w: gw - 4,
            h: gh - 4,
        }
    }

    pub fn division_cell(&self, index: usize) -> Rect {
        let n = 4i32;
        let w = self.divisions.w / n;
        Rect {
            x: self.divisions.x + (index as i32) * w + 2,
            y: self.divisions.y + 4,
            w: w - 4,
            h: self.divisions.h - 8,
        }
    }

    pub fn kaoss_cell(&self, col: usize, row_from_bottom: usize) -> Rect {
        let cw = self.kaoss.w / 12;
        let ch = self.kaoss.h / 7;
        let row_from_top = 6 - row_from_bottom as i32;
        Rect {
            x: self.kaoss.x + col as i32 * cw,
            y: self.kaoss.y + row_from_top * ch,
            w: cw,
            h: ch,
        }
    }

    pub fn hit(&self, px: i32, py: i32) -> Hit {
        if self.kaoss.contains(px, py) {
            let (x, y) = self.kaoss.pad_xy(px, py);
            return Hit::Kaoss { x, y };
        }
        if self.drums.contains(px, py) {
            for index in 0..16 {
                if self.drum_cell(index).contains(px, py) {
                    return Hit::Drum {
                        index,
                        note: 36 + index as u8,
                    };
                }
            }
            return Hit::None;
        }
        if self.divisions.contains(px, py) {
            for index in 0..4 {
                if self.division_cell(index).contains(px, py) {
                    return Hit::Division(index);
                }
            }
        }
        Hit::None
    }
}

/// Which performance surface a captured finger owns until lift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Surface {
    Kaoss,
    Drum { note: u8, repeat: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kick_is_bottom_left() {
        let layout = Layout::new();
        let cell = layout.drum_cell(0);
        match layout.hit(cell.x + 4, cell.y + 4) {
            Hit::Drum { note: 36, .. } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn kaoss_bottom_is_y_zero() {
        let layout = Layout::new();
        let Hit::Kaoss { x, y } = layout.hit(layout.kaoss.x + 10, layout.kaoss.y + layout.kaoss.h - 2)
        else {
            panic!("expected kaoss");
        };
        assert!(x < 0.1);
        assert!(y < 0.1);
    }

    #[test]
    fn five_independent_hits_are_possible() {
        let layout = Layout::new();
        let points = [
            (layout.kaoss.x + 20, layout.kaoss.y + 20),
            (layout.kaoss.x + 200, layout.kaoss.y + 100),
            (layout.drum_cell(0).x + 4, layout.drum_cell(0).y + 4),
            (layout.drum_cell(1).x + 4, layout.drum_cell(1).y + 4),
            (layout.drum_cell(4).x + 4, layout.drum_cell(4).y + 4),
        ];
        let mut kinds = Vec::new();
        for (x, y) in points {
            kinds.push(layout.hit(x, y));
        }
        assert!(matches!(kinds[0], Hit::Kaoss { .. }));
        assert!(matches!(kinds[1], Hit::Kaoss { .. }));
        assert!(matches!(kinds[2], Hit::Drum { note: 36, .. }));
        assert!(matches!(kinds[3], Hit::Drum { note: 37, .. }));
        assert!(matches!(kinds[4], Hit::Drum { note: 40, .. }));
    }
}
