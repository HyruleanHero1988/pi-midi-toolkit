//! Horizontal piano roll — time on X, pitch on Y.
//!
//! First written for the SONGS visualizer. Songs playback now uses falling
//! notes; keep this roll as the SEQ / DAW editing primitive.
//!
//! Typical wiring later:
//! 1. Flatten clip events with [`crate::song_viz::pair_notes`].
//! 2. Draw with [`crate::scene::draw_piano_roll`] / [`crate::scene::draw_piano_overview`].

use crate::layout::Rect;
use crate::phrases::PPQ;
use crate::song_viz::VizNote;

/// Beats of score visible ahead of the playhead.
pub const LOOKAHEAD_BEATS: f32 = 6.0;
/// Beats of already-played score kept left of the playhead.
pub const LOOKBEHIND_BEATS: f32 = 1.5;

/// Compact editor window: pad a unison, clamp very wide files to ~3 octaves
/// around the median so a SEQ lane stays readable on 800×480.
pub fn pitch_range(notes: &[VizNote]) -> (u8, u8) {
    let mut lo = 127u8;
    let mut hi = 0u8;
    for n in notes {
        lo = lo.min(n.note);
        hi = hi.max(n.note);
    }
    if hi < lo {
        return (48, 72);
    }
    let span = hi.saturating_sub(lo);
    if span < 12 {
        let mid = (lo as i32 + hi as i32) / 2;
        let a = (mid - 8).clamp(0, 127) as u8;
        let b = (a as i32 + 16).min(127) as u8;
        (a, b.max(a + 1))
    } else if span > 36 {
        let mid = (lo as i32 + hi as i32) / 2;
        let a = (mid - 18).clamp(0, 127) as u8;
        let b = (a as i32 + 36).min(127) as u8;
        (a, b.max(a + 1))
    } else {
        let a = lo.saturating_sub(1);
        let b = hi.saturating_add(1).min(127);
        (a, b.max(a + 1))
    }
}

pub fn window_ticks() -> (u32, u32) {
    let ahead = ((PPQ as f32) * LOOKAHEAD_BEATS).round() as u32;
    let behind = ((PPQ as f32) * LOOKBEHIND_BEATS).round() as u32;
    (behind, ahead)
}

pub fn playhead_x(roll: Rect) -> f32 {
    roll.x as f32 + roll.w as f32 * 0.22
}

/// Map a tick onto the rolling piano-roll X, given the current playhead.
pub fn tick_x(tick: i64, playhead: i64, roll: Rect) -> f32 {
    let (_behind, ahead) = window_ticks();
    let px = playhead_x(roll);
    let right = (roll.x + roll.w) as f32;
    let ppt = (right - px) / ahead.max(1) as f32;
    px + (tick - playhead) as f32 * ppt
}

/// Left gutter for a vertical keyboard beside the roll.
pub fn key_gutter(roll: Rect) -> Rect {
    Rect {
        x: roll.x,
        y: roll.y,
        w: 36,
        h: roll.h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_range_pads_a_unison() {
        let notes = [VizNote {
            start: 0,
            end: 100,
            note: 60,
            velocity: 100,
            channel: 0,
        }];
        let (lo, hi) = pitch_range(&notes);
        assert!(hi - lo >= 12);
        assert!(lo <= 60 && 60 <= hi);
    }

    #[test]
    fn wide_files_clamp_around_the_median() {
        let notes = [
            VizNote {
                start: 0,
                end: 10,
                note: 36,
                velocity: 80,
                channel: 9,
            },
            VizNote {
                start: 0,
                end: 10,
                note: 88,
                velocity: 80,
                channel: 0,
            },
        ];
        let (lo, hi) = pitch_range(&notes);
        assert!(hi - lo <= 36);
        assert!(lo < 62 && 62 < hi);
    }
}
