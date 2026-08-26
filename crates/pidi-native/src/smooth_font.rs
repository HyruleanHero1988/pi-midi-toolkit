//! Anti-aliased TTF atlas for the non-retro font style.

use std::sync::OnceLock;

use fontdue::{Font, FontSettings};

#[derive(Debug, Clone, Copy)]
pub struct SmoothGlyph {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub advance: f32,
    pub width: f32,
    pub height: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Debug)]
pub struct SmoothAtlas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub px_size: f32,
    pub line_height: f32,
    glyphs: Vec<Option<SmoothGlyph>>,
}

const FIRST: u8 = 32;
const COUNT: usize = 96;
const CELL: u32 = 32;
const COLS: u32 = 16;
const ROWS: u32 = 6;

const SYSTEM_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];

static SMOOTH: OnceLock<Option<SmoothAtlas>> = OnceLock::new();

fn load_ttf_bytes() -> Option<Vec<u8>> {
    for path in SYSTEM_FONT_PATHS {
        if let Ok(bytes) = std::fs::read(path) {
            if !bytes.is_empty() {
                return Some(bytes);
            }
        }
    }
    Some(include_bytes!("../assets/Roboto-Regular.ttf").to_vec())
}

pub fn smooth_atlas() -> Option<&'static SmoothAtlas> {
    SMOOTH
        .get_or_init(|| {
            let bytes = load_ttf_bytes()?;
            Some(SmoothAtlas::build(&bytes, 17.0))
        })
        .as_ref()
}

impl SmoothAtlas {
    pub fn build(ttf: &[u8], px_size: f32) -> Self {
        let font = Font::from_bytes(ttf, FontSettings::default()).expect("valid TTF");
        let width = COLS * CELL;
        let height = ROWS * CELL;
        let mut data = vec![0u8; (width * height * 4) as usize];

        let mut glyphs = vec![None; COUNT];
        for i in 0..COUNT {
            let ch = (FIRST + i as u8) as char;
            let (metrics, bitmap) = font.rasterize(ch, px_size);
            let col = (i as u32) % COLS;
            let row = (i as u32) / COLS;
            let ox = col * CELL + 2;
            let oy = row * CELL + CELL.saturating_sub(metrics.height as u32 + 4);

            for y in 0..metrics.height {
                for x in 0..metrics.width {
                    let alpha = bitmap[y * metrics.width + x];
                    if alpha == 0 {
                        continue;
                    }
                    let px = ox + x as u32;
                    let py = oy + y as u32;
                    if px >= width || py >= height {
                        continue;
                    }
                    let idx = ((py * width + px) * 4) as usize;
                    data[idx] = 255;
                    data[idx + 1] = 255;
                    data[idx + 2] = 255;
                    data[idx + 3] = alpha;
                }
            }

            let u0 = ox as f32 / width as f32;
            let v0 = oy as f32 / height as f32;
            let u1 = (ox + metrics.width as u32).min(width) as f32 / width as f32;
            let v1 = (oy + metrics.height as u32).min(height) as f32 / height as f32;
            glyphs[i] = Some(SmoothGlyph {
                u0,
                v0,
                u1,
                v1,
                advance: metrics.advance_width,
                width: metrics.width as f32,
                height: metrics.height as f32,
                x_offset: metrics.xmin as f32,
                y_offset: metrics.ymin as f32,
            });
        }

        let line_height = font
            .horizontal_line_metrics(px_size)
            .map(|m| m.new_line_size)
            .unwrap_or(px_size * 1.25);

        Self {
            width,
            height,
            data,
            px_size,
            line_height,
            glyphs,
        }
    }

    pub fn glyph(&self, ch: char) -> SmoothGlyph {
        let idx = smooth_index(ch);
        self.glyphs[idx]
            .unwrap_or_else(|| self.glyphs[0].expect("space glyph"))
    }

    pub fn atlas_rgba(&self) -> (u32, u32, &[u8]) {
        (self.width, self.height, &self.data)
    }
}

fn smooth_index(ch: char) -> usize {
    let mut c = ch as u8;
    if c >= b'a' && c <= b'z' {
        c -= 32;
    }
    if (FIRST..FIRST + COUNT as u8).contains(&c) {
        (c - FIRST) as usize
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_atlas_builds() {
        let atlas = smooth_atlas().expect("smooth atlas");
        assert!(atlas.width > 0);
        assert!(atlas.data.chunks(4).any(|p| p[3] > 0));
        let g = atlas.glyph('A');
        assert!(g.advance > 0.0);
    }
}
