//! Anti-aliased TTF atlas for the non-retro font style.
//!
//! Design contract with the rest of the UI:
//! - Call sites pass the same integer `scale` they use for retro (1 / 2 / 3).
//! - Smooth maps that to a **pixel height** comparable to retro (`scale * 7`),
//!   not "multiply the baked atlas by scale" (which blew out every button).
//! - Glyphs share one baseline via fontdue ascent + `ymin` (bitmap bottom
//!   relative to baseline). Screen Y grows downward.

use std::sync::OnceLock;

use fontdue::{Font, FontSettings};

#[derive(Debug, Clone, Copy)]
pub struct SmoothGlyph {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    /// Horizontal advance in bake-space pixels.
    pub advance: f32,
    pub width: f32,
    pub height: f32,
    /// Bitmap left edge relative to pen (fontdue `xmin`).
    pub xmin: f32,
    /// Bitmap bottom edge relative to baseline (fontdue `ymin`, often negative).
    pub ymin: f32,
}

#[derive(Debug)]
pub struct SmoothAtlas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Pixel size used when rasterizing into the atlas.
    pub px_size: f32,
    /// Distance from baseline up to the typographic ascender (bake space).
    pub ascent: f32,
    /// Distance from baseline down to the descender; typically negative.
    pub descent: f32,
    /// Recommended line box height (`ascent - descent`).
    pub line_height: f32,
    glyphs: Vec<Option<SmoothGlyph>>,
}

const FIRST: u8 = 32;
const COUNT: usize = 96;
/// Cell must fit bake glyphs with padding; 28px bake needs ~32.
const CELL: u32 = 40;
const COLS: u32 = 16;
const ROWS: u32 = 6;
/// Bake larger than on-screen so GLES linear filter looks clean when downscaled.
const BAKE_PX: f32 = 28.0;

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
            Some(SmoothAtlas::build(&bytes, BAKE_PX))
        })
        .as_ref()
}

/// Map UI `scale` (same ints as retro) to an on-screen em size in pixels.
///
/// Retro scale N draws an N×7 bitmap cell. Smooth targets that height so
/// existing `text_centered(..., 2)` call sites keep fitting buttons.
pub fn smooth_target_px(ui_scale: i32) -> f32 {
    let n = ui_scale.max(1);
    // Floor at ~12px — TTF at true 7px is muddy on the 7″ panel.
    ((n * 7) as f32).max(12.0)
}

impl SmoothAtlas {
    pub fn build(ttf: &[u8], px_size: f32) -> Self {
        let font = Font::from_bytes(ttf, FontSettings::default()).expect("valid TTF");
        let width = COLS * CELL;
        let height = ROWS * CELL;
        let mut data = vec![0u8; (width * height * 4) as usize];

        let line = font
            .horizontal_line_metrics(px_size)
            .expect("horizontal line metrics");
        let ascent = line.ascent;
        let descent = line.descent;
        let line_height = ascent - descent;

        // Baseline row inside each atlas cell (from top of cell).
        let cell_baseline = (ascent + 2.0).round().clamp(2.0, (CELL - 4) as f32) as u32;

        let mut glyphs = vec![None; COUNT];
        for i in 0..COUNT {
            let ch = (FIRST + i as u8) as char;
            let (metrics, bitmap) = font.rasterize(ch, px_size);
            let col = (i as u32) % COLS;
            let row = (i as u32) / COLS;
            let cell_x = col * CELL;
            let cell_y = row * CELL;

            // Place bitmap so its bottom sits at ymin relative to cell baseline.
            // Screen/atlas Y grows down; font ymin is bottom edge vs baseline.
            let bmp_w = metrics.width as u32;
            let bmp_h = metrics.height as u32;
            let ox = (cell_x as i32 + 2 + metrics.xmin).clamp(cell_x as i32, (cell_x + CELL - 1) as i32)
                as u32;
            // top = baseline - (ymin + height)
            let top = cell_baseline as i32 - (metrics.ymin + metrics.height as i32);
            let oy = top.clamp(cell_y as i32, (cell_y + CELL - 1) as i32) as u32;

            for y in 0..metrics.height {
                for x in 0..metrics.width {
                    let alpha = bitmap[y * metrics.width + x];
                    if alpha == 0 {
                        continue;
                    }
                    let px = ox + x as u32;
                    let py = oy + y as u32;
                    if px >= cell_x + CELL || py >= cell_y + CELL || px >= width || py >= height {
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
            let u1 = (ox + bmp_w).min(width) as f32 / width as f32;
            let v1 = (oy + bmp_h).min(height) as f32 / height as f32;
            glyphs[i] = Some(SmoothGlyph {
                u0,
                v0,
                u1,
                v1,
                advance: metrics.advance_width.max(1.0),
                width: bmp_w as f32,
                height: bmp_h as f32,
                xmin: metrics.xmin as f32,
                ymin: metrics.ymin as f32,
            });
        }

        Self {
            width,
            height,
            data,
            px_size,
            ascent,
            descent,
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

    /// Scale factor from bake space → on-screen pixels for a UI scale tier.
    pub fn draw_scale(&self, ui_scale: i32) -> f32 {
        smooth_target_px(ui_scale) / self.px_size
    }

    pub fn measure(&self, text: &str, ui_scale: i32) -> (i32, i32) {
        let s = self.draw_scale(ui_scale);
        let width = text
            .chars()
            .map(|ch| (self.glyph(ch).advance * s).round() as i32)
            .sum::<i32>()
            .max(1);
        // UI labels are uppercase. Center on the ascender box that `text_scaled_smooth`
        // uses for `y` → baseline, not full line_height (descender padding shifts caps up).
        let height = (self.ascent * s).round().max(1.0) as i32;
        (width, height)
    }
}

fn smooth_index(ch: char) -> usize {
    let mut c = ch as u8;
    if (b'a'..=b'z').contains(&c) {
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
        assert!(atlas.ascent > 0.0);
        assert!(atlas.descent <= 0.0);
    }

    #[test]
    fn ui_scale_two_matches_retro_button_height() {
        // Retro scale 2 → 14px. Smooth should target the same band.
        let px = smooth_target_px(2);
        assert!((px - 14.0).abs() < 0.01);
        let atlas = smooth_atlas().expect("smooth atlas");
        let s = atlas.draw_scale(2);
        let h = atlas.ascent * s;
        assert!(
            h < 20.0,
            "scale-2 ascender box should fit a 40–48px button, got {h}"
        );
        assert!(h > 8.0, "scale-2 must stay readable, got {h}");
    }

    #[test]
    fn capital_glyphs_share_a_baseline() {
        let atlas = smooth_atlas().expect("smooth atlas");
        let s = atlas.draw_scale(2);
        let line_top = 100.0_f32;
        let baseline = line_top + atlas.ascent * s;
        let mut bottoms = Vec::new();
        for ch in ['A', 'B', 'E', 'H', 'X'] {
            let g = atlas.glyph(ch);
            let bottom = baseline - g.ymin * s;
            bottoms.push(bottom);
        }
        let min = bottoms.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = bottoms.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (max - min) < 1.5,
            "capital baselines drifted: {bottoms:?} delta {}",
            max - min
        );
    }
}