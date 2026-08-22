//! CPU complete-frame renderer. The previous frame stays visible until this
//! one is fully filled and presented.

use crate::font::{self, GLYPH_H, GLYPH_STRIDE, GLYPH_W};
use crate::model::{NativeModel, RepeatDivisionChoice, LED_COLS, LED_ROWS};

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
    frame.clear(0x101018);
    let layout = model.layout;

    frame.fill_rect(layout.hud.x, layout.hud.y, layout.hud.w, layout.hud.h, 0x1c1c28);
    let hud = format!(
        "PIDI NATIVE  fps {:>4.0}  cb {}/{}us  xrun {}  drop {}  rel {}  xy {}  rpt {}  {}",
        model.fps,
        model.status.callback_frames,
        model.status.callback_micros,
        model.status.xruns,
        model.status.command_drops,
        model.status.emergency_releases,
        model.status.touch_overwrites,
        model.status.active_repeats,
        if model.connected { "ENG" } else { "NO ENGINE" },
    );
    frame.text(8, 14, &hud, 0xf2f2f2);

    // One pass over the 12×7 field — not 84 widget mutations.
    for row in 0..LED_ROWS {
        for col in 0..LED_COLS {
            let cell = layout.kaoss_cell(col, row);
            frame.fill_rect(cell.x, cell.y, cell.w - 1, cell.h - 1, model.cell(col, row));
        }
    }

    const DRUM_LABELS: [&str; 16] = [
        "KICK", "SNARE", "CLAP", "CHH", "OHH", "TOM L", "TOM M", "RIM", "KIK2", "RIM2", "SHKR",
        "PED", "TOM H", "COW", "CLV", "RIDE",
    ];
    for index in 0..16 {
        let cell = layout.drum_cell(index);
        let color = if index == 0 { 0x3a2030 } else { 0x242436 };
        frame.fill_rect(cell.x, cell.y, cell.w, cell.h, color);
        frame.text(cell.x + 6, cell.y + cell.h / 2 - 3, DRUM_LABELS[index], 0xd0d0e0);
    }

    for index in 0..4 {
        let cell = layout.division_cell(index);
        let choice = RepeatDivisionChoice::from_index(index);
        let on = choice == model.division;
        frame.fill_rect(
            cell.x,
            cell.y,
            cell.w,
            cell.h,
            if on { 0x5a3060 } else { 0x20202c },
        );
        frame.text(cell.x + 8, cell.y + 16, choice.label(), 0xffffff);
    }

    frame.fill_rect(
        layout.footer.x,
        layout.footer.y,
        layout.footer.w,
        layout.footer.h,
        0x1c1c28,
    );
    frame.text(
        8,
        448,
        "KAOSS LEFT  DRUMS RIGHT  HOLD KICK TO REPEAT  ENGINE OWNS TIME",
        0xa0a0b8,
    );
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
        model.tick(1.0 / 60.0);
        let mut frame = Frame::new();
        draw(&mut frame, &model);
        assert_eq!(frame.pixels.len(), SCREEN_W * SCREEN_H);
        let lit = frame.pixels.iter().filter(|p| **p != 0x101018).count();
        assert!(lit > 10_000);
    }
}
