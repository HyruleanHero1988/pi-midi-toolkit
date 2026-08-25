//! 800×480 hit-testing. Coordinates are pixels unless noted as 0..1 pad space.

use crate::mode::UiMode;

pub const SCREEN_W: i32 = 800;
pub const SCREEN_H: i32 = 480;
pub const NAV_H: i32 = 48;
pub const HUD_H: i32 = 36;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Nav(UiMode),
    Kaoss { x: f32, y: f32 },
    Drum { index: usize, note: u8 },
    Division(usize),
    PhrasePad(usize),
    StopAllClips,
    HomeTile(UiMode),
    SynthSlider(usize),
    SynthKey(usize),
    KaossScale,
    KaossKey,
    KaossFull,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
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
    pub content: Rect,
    pub nav: Rect,
    pub kaoss: Rect,
    pub drums: Rect,
    pub divisions: Rect,
    pub phrase_grid: Rect,
    pub stop_all: Rect,
    pub synth_sliders: Rect,
    pub synth_keys: Rect,
    pub kaoss_scale: Rect,
    pub kaoss_key: Rect,
    pub kaoss_full: Rect,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

impl Layout {
    pub fn new() -> Self {
        let content_h = SCREEN_H - HUD_H - NAV_H;
        Self {
            hud: Rect {
                x: 0,
                y: 0,
                w: SCREEN_W,
                h: HUD_H,
            },
            content: Rect {
                x: 0,
                y: HUD_H,
                w: SCREEN_W,
                h: content_h,
            },
            nav: Rect {
                x: 0,
                y: SCREEN_H - NAV_H,
                w: SCREEN_W,
                h: NAV_H,
            },
            kaoss: Rect {
                x: 8,
                y: HUD_H + 8,
                w: 520,
                h: content_h - 16,
            },
            drums: Rect {
                x: 540,
                y: HUD_H + 8,
                w: 252,
                h: 252,
            },
            divisions: Rect {
                x: 540,
                y: HUD_H + 268,
                w: 252,
                h: 52,
            },
            phrase_grid: Rect {
                x: 16,
                y: HUD_H + 16,
                w: 640,
                h: content_h - 80,
            },
            stop_all: Rect {
                x: 668,
                y: HUD_H + 16,
                w: 116,
                h: 64,
            },
            synth_sliders: Rect {
                x: 24,
                y: HUD_H + 24,
                w: 752,
                h: 220,
            },
            synth_keys: Rect {
                x: 24,
                y: HUD_H + 260,
                w: 752,
                h: content_h - 280,
            },
            kaoss_scale: Rect {
                x: 540,
                y: HUD_H + 368,
                w: 80,
                h: 40,
            },
            kaoss_key: Rect {
                x: 624,
                y: HUD_H + 368,
                w: 80,
                h: 40,
            },
            kaoss_full: Rect {
                x: 708,
                y: HUD_H + 368,
                w: 84,
                h: 40,
            },
        }
    }

    /// Performance layout when FULL PAD hides the drum chrome.
    pub fn apply_kaoss_full(&mut self, full: bool) {
        let content_h = SCREEN_H - HUD_H - NAV_H;
        if full {
            self.kaoss = Rect {
                x: 8,
                y: HUD_H + 8,
                w: SCREEN_W - 16,
                h: content_h - 56,
            };
            self.drums = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.divisions = Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            self.kaoss_scale = Rect {
                x: 8,
                y: HUD_H + content_h - 44,
                w: 120,
                h: 40,
            };
            self.kaoss_key = Rect {
                x: 136,
                y: HUD_H + content_h - 44,
                w: 120,
                h: 40,
            };
            self.kaoss_full = Rect {
                x: 264,
                y: HUD_H + content_h - 44,
                w: 120,
                h: 40,
            };
        } else {
            *self = Self::new();
        }
    }

    pub fn nav_cell(&self, index: usize) -> Rect {
        let n = UiMode::ALL.len() as i32;
        let w = self.nav.w / n;
        Rect {
            x: self.nav.x + (index as i32) * w + 1,
            y: self.nav.y + 4,
            w: w - 2,
            h: self.nav.h - 8,
        }
    }

    pub fn home_tile(&self, index: usize) -> Rect {
        let cols = 3i32;
        let rows = 3i32;
        let gw = (self.content.w - 24) / cols;
        let gh = (self.content.h - 24) / rows;
        let col = (index as i32) % cols;
        let row = (index as i32) / cols;
        Rect {
            x: self.content.x + 12 + col * gw + 4,
            y: self.content.y + 12 + row * gh + 4,
            w: gw - 8,
            h: gh - 8,
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

    pub fn phrase_cell(&self, index: usize) -> Rect {
        let col = (index % 4) as i32;
        let row = (index / 4) as i32;
        let gw = self.phrase_grid.w / 4;
        let gh = self.phrase_grid.h / 4;
        Rect {
            x: self.phrase_grid.x + col * gw + 3,
            y: self.phrase_grid.y + row * gh + 3,
            w: gw - 6,
            h: gh - 6,
        }
    }

    pub fn synth_slider(&self, index: usize) -> Rect {
        let n = 5i32;
        let w = self.synth_sliders.w / n;
        Rect {
            x: self.synth_sliders.x + (index as i32) * w + 6,
            y: self.synth_sliders.y + 28,
            w: w - 12,
            h: self.synth_sliders.h - 36,
        }
    }

    pub fn synth_key(&self, index: usize) -> Rect {
        let n = 12i32;
        let w = self.synth_keys.w / n;
        Rect {
            x: self.synth_keys.x + (index as i32) * w + 2,
            y: self.synth_keys.y + 4,
            w: w - 4,
            h: self.synth_keys.h - 8,
        }
    }

    pub fn hit(&self, mode: UiMode, px: i32, py: i32) -> Hit {
        if self.nav.contains(px, py) {
            for (i, m) in UiMode::ALL.iter().enumerate() {
                if self.nav_cell(i).contains(px, py) {
                    return Hit::Nav(*m);
                }
            }
            return Hit::None;
        }
        match mode {
            UiMode::Kaoss => self.hit_kaoss(px, py),
            UiMode::Pads => self.hit_pads(px, py),
            UiMode::Home => self.hit_home(px, py),
            UiMode::Synth => self.hit_synth(px, py),
            _ => Hit::None,
        }
    }

    fn hit_kaoss(&self, px: i32, py: i32) -> Hit {
        if self.kaoss_scale.contains(px, py) {
            return Hit::KaossScale;
        }
        if self.kaoss_key.contains(px, py) {
            return Hit::KaossKey;
        }
        if self.kaoss_full.contains(px, py) {
            return Hit::KaossFull;
        }
        if self.kaoss.contains(px, py) {
            let (x, y) = self.kaoss.pad_xy(px, py);
            return Hit::Kaoss { x, y };
        }
        if self.drums.contains(px, py) && self.drums.w > 0 {
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
        if self.divisions.contains(px, py) && self.divisions.w > 0 {
            for index in 0..4 {
                if self.division_cell(index).contains(px, py) {
                    return Hit::Division(index);
                }
            }
        }
        Hit::None
    }

    fn hit_pads(&self, px: i32, py: i32) -> Hit {
        if self.stop_all.contains(px, py) {
            return Hit::StopAllClips;
        }
        for index in 0..16 {
            if self.phrase_cell(index).contains(px, py) {
                return Hit::PhrasePad(index);
            }
        }
        Hit::None
    }

    fn hit_home(&self, px: i32, py: i32) -> Hit {
        for (i, mode) in UiMode::ALL.iter().enumerate() {
            if self.home_tile(i).contains(px, py) {
                return Hit::HomeTile(*mode);
            }
        }
        Hit::None
    }

    fn hit_synth(&self, px: i32, py: i32) -> Hit {
        for index in 0..5 {
            if self.synth_slider(index).contains(px, py) {
                return Hit::SynthSlider(index);
            }
        }
        for index in 0..12 {
            if self.synth_key(index).contains(px, py) {
                return Hit::SynthKey(index);
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
    Phrase { slot: usize },
    SynthKey { note: u8 },
    SynthSlider { index: usize },
    UiTap,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kick_is_bottom_left() {
        let layout = Layout::new();
        let cell = layout.drum_cell(0);
        match layout.hit(UiMode::Kaoss, cell.x + 4, cell.y + 4) {
            Hit::Drum { note: 36, .. } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn kaoss_bottom_is_y_zero() {
        let layout = Layout::new();
        let Hit::Kaoss { x, y } =
            layout.hit(UiMode::Kaoss, layout.kaoss.x + 10, layout.kaoss.y + layout.kaoss.h - 2)
        else {
            panic!("expected kaoss");
        };
        assert!(x < 0.1);
        assert!(y < 0.1);
    }

    #[test]
    fn nav_switches_modes() {
        let layout = Layout::new();
        let cell = layout.nav_cell(UiMode::Pads.index());
        assert_eq!(
            layout.hit(UiMode::Kaoss, cell.x + 4, cell.y + 4),
            Hit::Nav(UiMode::Pads)
        );
    }

    #[test]
    fn phrase_pad_a1_is_top_left() {
        let layout = Layout::new();
        let cell = layout.phrase_cell(0);
        assert_eq!(
            layout.hit(UiMode::Pads, cell.x + 4, cell.y + 4),
            Hit::PhrasePad(0)
        );
    }
}
