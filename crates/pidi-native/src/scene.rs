//! Batched draw list for the 800×480 surface.
//!
//! GLES consumes this as two meshes (color quads + glyph quads). The CPU
//! rasterizer uses the same list so dummy/PPM output matches the GPU path.

use crate::font::{self, GLYPH_H, GLYPH_STRIDE, GLYPH_W};
use crate::layout::Rect;
use crate::model::{NativeModel, RepeatDivisionChoice, LED_COLS, LED_ROWS};
use crate::render::{SCREEN_H, SCREEN_W};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphQuad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub color: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub clear: u32,
    pub color: Vec<ColorQuad>,
    pub glyphs: Vec<GlyphQuad>,
}

impl Scene {
    pub fn fill_rect(&mut self, rect: Rect, color: u32) {
        self.fill(rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32, color);
    }

    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.color.push(ColorQuad { x, y, w, h, color });
    }

    pub fn text(&mut self, mut x: i32, y: i32, s: &str, color: u32) {
        for ch in s.chars() {
            let (u0, v0, u1, v1) = font::glyph_uv(ch);
            self.glyphs.push(GlyphQuad {
                x: x as f32,
                y: y as f32,
                w: GLYPH_W as f32,
                h: GLYPH_H as f32,
                u0,
                v0,
                u1,
                v1,
                color,
            });
            x += GLYPH_STRIDE;
        }
    }
}

pub fn build(model: &NativeModel) -> Scene {
    let mut scene = Scene {
        clear: 0x101018,
        color: Vec::with_capacity(84 + 32),
        glyphs: Vec::with_capacity(160),
    };
    let layout = model.layout;
    scene.fill_rect(layout.hud, 0x1c1c28);
    scene.text(
        8,
        14,
        &format!(
            "PIDI NATIVE  GLES  fps {:>4.0}  cb {}/{}us  xrun {}  drop {}  rel {}  xy {}  rpt {}  {}",
            model.fps,
            model.status.callback_frames,
            model.status.callback_micros,
            model.status.xruns,
            model.status.command_drops,
            model.status.emergency_releases,
            model.status.touch_overwrites,
            model.status.active_repeats,
            if model.connected { "ENG" } else { "NO ENGINE" },
        ),
        0xf2f2f2,
    );

    for row in 0..LED_ROWS {
        for col in 0..LED_COLS {
            let cell = layout.kaoss_cell(col, row);
            scene.fill(
                cell.x as f32,
                cell.y as f32,
                (cell.w - 1) as f32,
                (cell.h - 1) as f32,
                model.cell(col, row),
            );
        }
    }

    const DRUM_LABELS: [&str; 16] = [
        "KICK", "SNARE", "CLAP", "CHH", "OHH", "TOM L", "TOM M", "RIM", "KIK2", "RIM2", "SHKR",
        "PED", "TOM H", "COW", "CLV", "RIDE",
    ];
    for index in 0..16 {
        let cell = layout.drum_cell(index);
        scene.fill_rect(cell, if index == 0 { 0x3a2030 } else { 0x242436 });
        scene.text(
            cell.x + 6,
            cell.y + cell.h / 2 - 3,
            DRUM_LABELS[index],
            0xd0d0e0,
        );
    }

    for index in 0..4 {
        let cell = layout.division_cell(index);
        let choice = RepeatDivisionChoice::from_index(index);
        scene.fill_rect(cell, if choice == model.division { 0x5a3060 } else { 0x20202c });
        scene.text(cell.x + 8, cell.y + 16, choice.label(), 0xffffff);
    }

    scene.fill_rect(layout.footer, 0x1c1c28);
    scene.text(
        8,
        448,
        "KAOSS LEFT  DRUMS RIGHT  HOLD KICK TO REPEAT  ENGINE OWNS TIME",
        0xa0a0b8,
    );
    let _ = (SCREEN_W, SCREEN_H);
    scene
}

pub fn unpack_rgb(color: u32) -> [f32; 4] {
    [
        ((color >> 16) & 0xff) as f32 / 255.0,
        ((color >> 8) & 0xff) as f32 / 255.0,
        (color & 0xff) as f32 / 255.0,
        1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Outbox;
    use crate::model::NativeModel;

    #[test]
    fn cells_are_one_batched_field() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let k = model.layout.kaoss;
        model.finger_down(1, k.x + 40, k.y + 40, &mut out);
        model.tick(1.0 / 60.0);
        let scene = build(&model);
        let cells = scene
            .color
            .iter()
            .filter(|q| {
                q.x >= model.layout.kaoss.x as f32
                    && q.y >= model.layout.kaoss.y as f32
                    && q.x < (model.layout.kaoss.x + model.layout.kaoss.w) as f32
                    && q.y < (model.layout.kaoss.y + model.layout.kaoss.h) as f32
                    && q.w < 80.0
                    && q.h < 80.0
            })
            .count();
        assert_eq!(cells, LED_COLS * LED_ROWS);
        assert!(!scene.glyphs.is_empty());
    }
}
