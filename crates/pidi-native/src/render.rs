//! CPU complete-frame renderer. The previous frame stays visible until this
//! one is fully filled and presented. Dummy/PPM output rasterizes the same
//! `scene::build` list that GLES submits, so the two paths stay aligned.

use crate::font::{self, GLYPH_H, GLYPH_STRIDE, GLYPH_W};
use crate::model::NativeModel;
use crate::scene::{self, Scene};

pub const SCREEN_W: usize = 800;
pub const SCREEN_H: usize = 480;

#[derive(Clone)]
pub struct Frame {
    pub pixels: Vec<u32>,
}

impl Frame {
    pub fn new() -> Self {
        Self {
            pixels: vec![0; SCREEN_W * SCREEN_H],
        }
    }

    pub fn clear(&mut self, color: u32) {
        self.pixels.fill(color);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = (x + w).clamp(0, SCREEN_W as i32) as usize;
        let y1 = (y + h).clamp(0, SCREEN_H as i32) as usize;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for row in y0..y1 {
            let start = row * SCREEN_W + x0;
            self.pixels[start..start + (x1 - x0)].fill(color);
        }
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, color: u32) {
        self.fill_rect(x, y, w, 1, color);
    }

    pub fn text(&mut self, mut x: i32, y: i32, s: &str, color: u32) {
        for ch in s.chars() {
            let glyph = font::glyph(ch);
            for col in 0..GLYPH_W {
                let bits = glyph[col as usize];
                for row in 0..GLYPH_H {
                    if bits & (1 << row) != 0 {
                        let px = x + col;
                        let py = y + row;
                        if px >= 0 && py >= 0 && px < SCREEN_W as i32 && py < SCREEN_H as i32 {
                            self.pixels[py as usize * SCREEN_W + px as usize] = color;
                        }
                    }
                }
            }
            x += GLYPH_STRIDE;
        }
    }

    pub fn write_ppm(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(path)?;
        writeln!(f, "P6 {SCREEN_W} {SCREEN_H} 255")?;
        let mut bytes = Vec::with_capacity(SCREEN_W * SCREEN_H * 3);
        for px in &self.pixels {
            bytes.push((px >> 16) as u8);
            bytes.push((px >> 8) as u8);
            bytes.push(*px as u8);
        }
        f.write_all(&bytes)
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

pub fn draw(frame: &mut Frame, model: &NativeModel) {
    rasterize(frame, &scene::build(model));
}

pub fn rasterize(frame: &mut Frame, scene: &Scene) {
    frame.clear(scene.clear);
    for q in &scene.color {
        frame.fill_rect(q.x as i32, q.y as i32, q.w as i32, q.h as i32, q.color);
    }
    let (aw, ah, atlas) = font::atlas_rgba_for(scene.font_style.resolved());
    for g in &scene.glyphs {
        blit_glyph(frame, g, aw, ah, &atlas);
    }
}

fn blit_glyph(frame: &mut Frame, g: &scene::GlyphQuad, aw: u32, ah: u32, atlas: &[u8]) {
    let x0 = g.x as i32;
    let y0 = g.y as i32;
    let w = g.w.max(1.0) as i32;
    let h = g.h.max(1.0) as i32;
    let (cr, cg, cb) = (
        ((g.color >> 16) & 0xff) as u32,
        ((g.color >> 8) & 0xff) as u32,
        (g.color & 0xff) as u32,
    );
    for dy in 0..h {
        for dx in 0..w {
            let u = g.u0 + (g.u1 - g.u0) * ((dx as f32 + 0.5) / w as f32);
            let v = g.v0 + (g.v1 - g.v0) * ((dy as f32 + 0.5) / h as f32);
            let ax = (u * aw as f32)
                .floor()
                .clamp(0.0, aw as f32 - 1.0) as u32;
            let ay = (v * ah as f32)
                .floor()
                .clamp(0.0, ah as f32 - 1.0) as u32;
            let idx = ((ay * aw + ax) * 4) as usize;
            let a = atlas[idx + 3] as u32;
            if a == 0 {
                continue;
            }
            let px = x0 + dx;
            let py = y0 + dy;
            if px < 0 || py < 0 || px >= SCREEN_W as i32 || py >= SCREEN_H as i32 {
                continue;
            }
            let di = py as usize * SCREEN_W + px as usize;
            if a >= 250 {
                frame.pixels[di] = g.color;
                continue;
            }
            let dst = frame.pixels[di];
            let dr = (dst >> 16) & 0xff;
            let dg = (dst >> 8) & 0xff;
            let db = dst & 0xff;
            let inv = 255 - a;
            let r = (cr * a + dr * inv) / 255;
            let gch = (cg * a + dg * inv) / 255;
            let b = (cb * a + db * inv) / 255;
            frame.pixels[di] = (r << 16) | (gch << 8) | b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Outbox;
    use crate::model::NativeModel;

    #[test]
    fn a_complete_frame_covers_the_panel() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let k = model.layout.kaoss;
        model.finger_down(1, k.x + 40, k.y + 40, &mut out);
        let mut out = crate::client::Outbox::new();
        model.tick(1.0 / 60.0, &mut out);
        let mut frame = Frame::new();
        draw(&mut frame, &model);
        assert_eq!(frame.pixels.len(), SCREEN_W * SCREEN_H);
        let lit = frame.pixels.iter().filter(|p| **p != 0x101018).count();
        assert!(lit > 10_000);
    }
}
