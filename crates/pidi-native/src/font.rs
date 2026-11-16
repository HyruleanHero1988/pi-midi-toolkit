//! 5×7 bitmap font, ASCII 32..95 plus [`BRAND_I`]. Packed as 5 columns of 7 bits.
//!
//! Both UI fonts fold `a-z` to capitals, so ordinary text can never show a
//! lowercase letter. The product name is the one exception: draw [`BRAND`]
//! ("PiDI") and the lowercase `i` is rendered instead of being folded.

use serde::{Deserialize, Serialize};

pub use crate::smooth_font::smooth_atlas;

pub const GLYPH_W: i32 = 5;
pub const GLYPH_H: i32 = 7;
pub const GLYPH_STRIDE: i32 = 6;

/// Private-use stand-in for the lowercase `i` in the product name. Everything
/// else keeps the all-caps fold; only this code point bypasses it.
pub const BRAND_I: char = '\u{E069}';
/// On-screen product name, exactly "PiDI".
pub const BRAND: &str = "P\u{E069}DI";

/// UI text style — retro bitmap (default) or anti-aliased TTF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FontStyle {
    #[default]
    Retro,
    Smooth,
}

impl FontStyle {
    pub fn cycle(self) -> Self {
        match self {
            Self::Retro => Self::Smooth,
            Self::Smooth => Self::Retro,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Retro => "RETRO",
            Self::Smooth => "SMOOTH",
        }
    }

    pub fn settings_color(self) -> u32 {
        match self {
            Self::Retro => 0x504945,
            Self::Smooth => 0x458588,
        }
    }

    /// Default `text()` scale for this style (same tier for both: ~14px).
    pub fn default_text_scale(self) -> i32 {
        2
    }

    pub fn uses_linear_filter(self) -> bool {
        matches!(self, Self::Smooth)
    }

    /// Effective style after checking smooth atlas availability.
    pub fn resolved(self) -> Self {
        if matches!(self, Self::Smooth) && smooth_atlas().is_none() {
            Self::Retro
        } else {
            self
        }
    }
}

const FONT: [&[u8; 5]; 65] = [
    &[0x00, 0x00, 0x00, 0x00, 0x00], // space
    &[0x00, 0x00, 0x5F, 0x00, 0x00], // !
    &[0x00, 0x07, 0x00, 0x07, 0x00], // "
    &[0x14, 0x7F, 0x14, 0x7F, 0x14], // #
    &[0x24, 0x2A, 0x7F, 0x2A, 0x12], // $
    &[0x23, 0x13, 0x08, 0x64, 0x62], // %
    &[0x36, 0x49, 0x55, 0x22, 0x50], // &
    &[0x00, 0x05, 0x03, 0x00, 0x00], // '
    &[0x00, 0x1C, 0x22, 0x41, 0x00], // (
    &[0x00, 0x41, 0x22, 0x1C, 0x00], // )
    &[0x14, 0x08, 0x3E, 0x08, 0x14], // *
    &[0x08, 0x08, 0x3E, 0x08, 0x08], // +
    &[0x00, 0x50, 0x30, 0x00, 0x00], // ,
    &[0x08, 0x08, 0x08, 0x08, 0x08], // -
    &[0x00, 0x60, 0x60, 0x00, 0x00], // .
    &[0x20, 0x10, 0x08, 0x04, 0x02], // /
    &[0x3E, 0x51, 0x49, 0x45, 0x3E], // 0
    &[0x00, 0x42, 0x7F, 0x40, 0x00], // 1
    &[0x42, 0x61, 0x51, 0x49, 0x46], // 2
    &[0x21, 0x41, 0x45, 0x4B, 0x31], // 3
    &[0x18, 0x14, 0x12, 0x7F, 0x10], // 4
    &[0x27, 0x45, 0x45, 0x45, 0x39], // 5
    &[0x3C, 0x4A, 0x49, 0x49, 0x30], // 6
    &[0x01, 0x71, 0x09, 0x05, 0x03], // 7
    &[0x36, 0x49, 0x49, 0x49, 0x36], // 8
    &[0x06, 0x49, 0x49, 0x29, 0x1E], // 9
    &[0x00, 0x36, 0x36, 0x00, 0x00], // :
    &[0x00, 0x56, 0x36, 0x00, 0x00], // ;
    &[0x08, 0x14, 0x22, 0x41, 0x00], // <
    &[0x14, 0x14, 0x14, 0x14, 0x14], // =
    &[0x00, 0x41, 0x22, 0x14, 0x08], // >
    &[0x02, 0x01, 0x51, 0x09, 0x06], // ?
    &[0x32, 0x49, 0x79, 0x41, 0x3E], // @
    &[0x7E, 0x11, 0x11, 0x11, 0x7E], // A
    &[0x7F, 0x49, 0x49, 0x49, 0x36], // B
    &[0x3E, 0x41, 0x41, 0x41, 0x22], // C
    &[0x7F, 0x41, 0x41, 0x22, 0x1C], // D
    &[0x7F, 0x49, 0x49, 0x49, 0x41], // E
    &[0x7F, 0x09, 0x09, 0x09, 0x01], // F
    &[0x3E, 0x41, 0x49, 0x49, 0x7A], // G
    &[0x7F, 0x08, 0x08, 0x08, 0x7F], // H
    &[0x00, 0x41, 0x7F, 0x41, 0x00], // I
    &[0x20, 0x40, 0x41, 0x3F, 0x01], // J
    &[0x7F, 0x08, 0x14, 0x22, 0x41], // K
    &[0x7F, 0x40, 0x40, 0x40, 0x40], // L
    &[0x7F, 0x02, 0x0C, 0x02, 0x7F], // M
    &[0x7F, 0x04, 0x08, 0x10, 0x7F], // N
    &[0x3E, 0x41, 0x41, 0x41, 0x3E], // O
    &[0x7F, 0x09, 0x09, 0x09, 0x06], // P
    &[0x3E, 0x41, 0x51, 0x21, 0x5E], // Q
    &[0x7F, 0x09, 0x19, 0x29, 0x46], // R
    &[0x46, 0x49, 0x49, 0x49, 0x31], // S
    &[0x01, 0x01, 0x7F, 0x01, 0x01], // T
    &[0x3F, 0x40, 0x40, 0x40, 0x3F], // U
    &[0x1F, 0x20, 0x40, 0x20, 0x1F], // V
    &[0x3F, 0x40, 0x38, 0x40, 0x3F], // W
    &[0x63, 0x14, 0x08, 0x14, 0x63], // X
    &[0x07, 0x08, 0x70, 0x08, 0x07], // Y
    &[0x61, 0x51, 0x49, 0x45, 0x43], // Z
    &[0x00, 0x7F, 0x41, 0x41, 0x00], // [
    &[0x02, 0x04, 0x08, 0x10, 0x20], // \
    &[0x00, 0x41, 0x41, 0x7F, 0x00], // ]
    &[0x04, 0x02, 0x01, 0x02, 0x04], // ^
    &[0x40, 0x40, 0x40, 0x40, 0x40], // _
    &[0x00, 0x44, 0x7D, 0x40, 0x00], // BRAND_I (lowercase i, dot + serifed stem)
];

const BRAND_I_SLOT: usize = 64;

/// Atlas slot for ASCII 32..95, or `None` if the 5×7 font cannot draw `ch`.
///
/// Must use `u32` (not `char as u8`): U+232B ERASE TO THE LEFT (`⌫`) has
/// low byte 0x2B, which is `'+'`.
fn font_index(ch: char) -> Option<usize> {
    if ch == BRAND_I {
        return Some(BRAND_I_SLOT);
    }
    let mut code = ch as u32;
    if (b'a' as u32..=b'z' as u32).contains(&code) {
        code -= 32;
    }
    if (32..96).contains(&code) {
        Some((code - 32) as usize)
    } else {
        None
    }
}

pub fn glyph(ch: char) -> &'static [u8; 5] {
    font_index(ch).map(|i| FONT[i]).unwrap_or(FONT[0])
}

pub const ATLAS_COLS: u32 = 16;
pub const ATLAS_ROWS: u32 = 5;
pub const ATLAS_CELL: u32 = 8;

pub fn atlas_size() -> (u32, u32) {
    (ATLAS_COLS * ATLAS_CELL, ATLAS_ROWS * ATLAS_CELL)
}

/// White-on-transparent RGBA atlas for GLES text. One 8×8 cell per ASCII 32..95 plus the brand `i`.
pub fn atlas_rgba() -> (u32, u32, Vec<u8>) {
    let (w, h) = atlas_size();
    let mut data = vec![0u8; (w * h * 4) as usize];
    for i in 0..FONT.len() {
        let bits = FONT[i];
        let col = (i as u32) % ATLAS_COLS;
        let row = (i as u32) / ATLAS_COLS;
        let ox = col * ATLAS_CELL + 1;
        let oy = row * ATLAS_CELL + 1;
        for gx in 0..GLYPH_W as u32 {
            let column = bits[gx as usize];
            for gy in 0..GLYPH_H as u32 {
                if column & (1 << gy) != 0 {
                    let px = ox + gx;
                    let py = oy + gy;
                    let idx = ((py * w + px) * 4) as usize;
                    data[idx] = 255;
                    data[idx + 1] = 255;
                    data[idx + 2] = 255;
                    data[idx + 3] = 255;
                }
            }
        }
    }
    (w, h, data)
}

pub fn glyph_uv(ch: char) -> (f32, f32, f32, f32) {
    let i = font_index(ch).unwrap_or(0) as u32;
    let col = i % ATLAS_COLS;
    let row = i / ATLAS_COLS;
    let (w, h) = atlas_size();
    let ox = col * ATLAS_CELL + 1;
    let oy = row * ATLAS_CELL + 1;
    let u0 = ox as f32 / w as f32;
    let v0 = oy as f32 / h as f32;
    let u1 = (ox + GLYPH_W as u32) as f32 / w as f32;
    let v1 = (oy + GLYPH_H as u32) as f32 / h as f32;
    (u0, v0, u1, v1)
}

pub fn atlas_rgba_for(style: FontStyle) -> (u32, u32, Vec<u8>) {
    match style.resolved() {
        FontStyle::Smooth => {
            let atlas = smooth_atlas().expect("smooth atlas");
            let (w, h, data) = atlas.atlas_rgba();
            (w, h, data.to_vec())
        }
        FontStyle::Retro => atlas_rgba(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_has_ink_for_a() {
        let (w, h, data) = atlas_rgba();
        assert_eq!((w, h), (128, 40));
        assert!(data.chunks(4).any(|p| p[3] > 0));
    }

    #[test]
    fn brand_i_is_lowercase_and_ordinary_text_stays_caps() {
        assert_ne!(glyph(BRAND_I), glyph('I'));
        assert_ne!(glyph(BRAND_I), FONT[0]);
        assert_eq!(glyph('i'), glyph('I'), "plain lowercase still folds to caps");
        assert_ne!(glyph_uv(BRAND_I), glyph_uv('I'));
        let (w, h, data) = atlas_rgba();
        let (u0, v0, u1, v1) = glyph_uv(BRAND_I);
        let (x0, y0) = ((u0 * w as f32).round() as u32, (v0 * h as f32).round() as u32);
        let (x1, y1) = ((u1 * w as f32).round() as u32, (v1 * h as f32).round() as u32);
        assert!(y1 <= h && x1 <= w);
        let ink = (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|(x, y)| data[((y * w + x) * 4 + 3) as usize] > 0)
            .count();
        assert!(ink > 8, "brand i must have ink in the atlas, ink={ink}");
    }

    #[test]
    fn brand_text_is_pidi_and_four_cells_wide() {
        let shown: String = BRAND.chars().map(|c| if c == BRAND_I { 'i' } else { c }).collect();
        assert_eq!(shown, "PiDI");
        assert_eq!(BRAND.chars().count(), 4);
    }

    #[test]
    fn unicode_erase_does_not_alias_to_plus() {
        // U+232B's low byte is 0x2B ('+'). Truncating char→u8 used to draw '+'
        // for the backspace key.
        assert_ne!(glyph('⌫'), glyph('+'));
        assert_eq!(glyph('⌫'), glyph(' '));
        assert_ne!(glyph_uv('⌫'), glyph_uv('+'));
    }
}
