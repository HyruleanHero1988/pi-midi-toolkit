//! Batched draw list for the 800×480 surface.
//!
//! GLES consumes this as two meshes (color quads + glyph quads). The CPU
//! rasterizer uses the same list so dummy/PPM output matches the GPU path.

use crate::font::{self, GLYPH_H, GLYPH_STRIDE, GLYPH_W};
use crate::layout::Rect;
use crate::mode::UiMode;
use crate::model::{NativeModel, RepeatDivisionChoice, LED_COLS, LED_ROWS};
use crate::phrases;
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
        color: Vec::with_capacity(200),
        glyphs: Vec::with_capacity(240),
    };
    draw_chrome(&mut scene, model);
    match model.mode {
        UiMode::Kaoss => draw_kaoss(&mut scene, model),
        UiMode::Pads => draw_pads(&mut scene, model),
        UiMode::Home => draw_home(&mut scene, model),
        UiMode::Synth => draw_synth(&mut scene, model),
        other => draw_placeholder(&mut scene, model, other),
    }
    let _ = (SCREEN_W, SCREEN_H);
    scene
}

fn draw_chrome(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.hud, 0x1c1c28);
    let hud = format!(
        "{}  fps {:>4.0}  cb {}/{}us  xrun {}  {}",
        model.mode.title(),
        model.fps,
        model.status.callback_frames,
        model.status.callback_micros,
        model.status.xruns,
        if model.connected { "ENG" } else { "NO ENGINE" },
    );
    scene.text(8, 12, &hud, 0xf2f2f2);
    if !model.status_line.is_empty() {
        scene.text(420, 12, &model.status_line, 0xa0a0b8);
    }

    scene.fill_rect(layout.nav, 0x14141c);
    for (i, mode) in UiMode::ALL.iter().enumerate() {
        let cell = layout.nav_cell(i);
        let active = *mode == model.mode;
        scene.fill_rect(cell, if active { 0x5a3060 } else { 0x242430 });
        scene.text(
            cell.x + 6,
            cell.y + cell.h / 2 - 3,
            mode.label(),
            if active { 0xffffff } else { 0xb0b0c0 },
        );
    }
}

fn draw_kaoss(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
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

    if layout.drums.w > 0 {
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
            scene.fill_rect(
                cell,
                if choice == model.division {
                    0x5a3060
                } else {
                    0x20202c
                },
            );
            scene.text(cell.x + 8, cell.y + 16, choice.label(), 0xffffff);
        }
    }

    let scale = jambox_core::kaoss_scale(model.kaoss_scale_index as usize);
    scene.fill_rect(layout.kaoss_scale, 0x3a3048);
    scene.text(layout.kaoss_scale.x + 6, layout.kaoss_scale.y + 14, scale.label, 0xffffff);
    scene.fill_rect(layout.kaoss_key, 0x3a3048);
    scene.text(
        layout.kaoss_key.x + 16,
        layout.kaoss_key.y + 14,
        jambox_core::NOTE_NAMES[model.kaoss_key as usize],
        0xffffff,
    );
    scene.fill_rect(
        layout.kaoss_full,
        if model.kaoss_full { 0x5a3060 } else { 0x3a3048 },
    );
    scene.text(
        layout.kaoss_full.x + 10,
        layout.kaoss_full.y + 14,
        if model.kaoss_full { "EXIT" } else { "FULL" },
        0xffffff,
    );
}

fn draw_pads(scene: &mut Scene, model: &NativeModel) {
    let layout = model.layout;
    scene.fill_rect(layout.stop_all, 0x3c3836);
    scene.text(layout.stop_all.x + 10, layout.stop_all.y + 24, "STOP", 0xffffff);
    scene.text(layout.stop_all.x + 10, layout.stop_all.y + 40, "ALL", 0xffffff);

    for index in 0..16 {
        let cell = layout.phrase_cell(index);
        let pad = &model.phrases[index];
        let color = if model.phrase_playing[index] {
            0x689d6a
        } else if pad.empty {
            0x3c3836
        } else if pad.loop_mode {
            0x458588
        } else {
            0x076678
        };
        scene.fill_rect(cell, color);
        let label = phrases::pad_label(index);
        scene.text(cell.x + 10, cell.y + 16, &label, 0xfbf1c7);
        if !pad.empty {
            scene.text(
                cell.x + 10,
                cell.y + 36,
                if pad.loop_mode { "LOOP" } else { "1SHOT" },
                0xd5c4a1,
            );
        }
    }
}

fn draw_home(scene: &mut Scene, model: &NativeModel) {
    for (i, mode) in UiMode::ALL.iter().enumerate() {
        let cell = model.layout.home_tile(i);
        scene.fill_rect(cell, 0x282838);
        scene.text(
            cell.x + 16,
            cell.y + cell.h / 2 - 4,
            mode.title(),
            0xf2f2f2,
        );
    }
}

fn draw_synth(scene: &mut Scene, model: &NativeModel) {
    const LABELS: [&str; 5] = ["MORPH", "TONE", "LEVEL", "ATK", "REL"];
    const KEYS: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    for index in 0..5 {
        let track = model.layout.synth_slider(index);
        scene.fill_rect(track, 0x20202c);
        scene.text(track.x + 8, track.y - 18, LABELS[index], 0xc0c0d0);
        let fill_h = (track.h as f32 * model.synth_params[index]) as i32;
        let fill = Rect {
            x: track.x + 4,
            y: track.y + track.h - fill_h,
            w: track.w - 8,
            h: fill_h,
        };
        scene.fill_rect(fill, 0x5a3060);
    }
    for index in 0..12 {
        let key = model.layout.synth_key(index);
        let black = KEYS[index].contains('#');
        scene.fill_rect(key, if black { 0x1a1a22 } else { 0x3a3a48 });
        scene.text(key.x + 10, key.y + key.h / 2 - 3, KEYS[index], 0xf2f2f2);
    }
}

fn draw_placeholder(scene: &mut Scene, model: &NativeModel, mode: UiMode) {
    let c = model.layout.content;
    scene.text(
        c.x + 24,
        c.y + 48,
        &format!("{} — coming on this branch", mode.title()),
        0xc0c0d0,
    );
    scene.text(
        c.x + 24,
        c.y + 72,
        "Native kiosk port; engine owns musical time.",
        0x808098,
    );
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

    #[test]
    fn pads_mode_draws_sixteen_tiles() {
        let mut model = NativeModel::new();
        model.set_mode(UiMode::Pads);
        let scene = build(&model);
        let tiles = scene
            .color
            .iter()
            .filter(|q| {
                q.w > 100.0
                    && q.h > 50.0
                    && q.x >= model.layout.phrase_grid.x as f32
                    && q.y >= model.layout.phrase_grid.y as f32
            })
            .count();
        assert!(tiles >= 16);
    }
}
