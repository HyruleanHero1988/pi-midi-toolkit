//! Song playback visualizer: falling notes onto a keyboard + drum pads.
//!
//! The older horizontal roll lives in [`crate::piano_roll`] for SEQ / DAW work.

use jambox_protocol::WireClipEvent;

use crate::kaoss_viz;
use crate::layout::Rect;
use crate::phrases::PPQ;

/// One sounding note after pairing SMF note-on / note-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VizNote {
    pub start: u32,
    pub end: u32,
    pub note: u8,
    pub velocity: u8,
    pub channel: u8,
}

impl VizNote {
    pub fn duration(self) -> u32 {
        self.end.saturating_sub(self.start).max(1)
    }

    pub fn is_drum(self) -> bool {
        self.channel == 9
    }

    pub fn sounding_at(self, tick: u32) -> bool {
        tick >= self.start && tick < self.end
    }
}

/// Pair clip events into held notes. A second on of the same key closes the first.
pub fn pair_notes(events: &[WireClipEvent], length_ticks: u32) -> Vec<VizNote> {
    let mut open: Vec<(u32, u8, u8, u8)> = Vec::new();
    let mut out = Vec::new();
    let length = length_ticks.max(1);

    for ev in events {
        if ev.touch.is_some() {
            continue;
        }
        let ch = ev.channel & 0x0f;
        let note = ev.note & 0x7f;
        if ev.on && ev.velocity > 0 {
            if let Some(idx) = open.iter().position(|(_, c, n, _)| *c == ch && *n == note) {
                let (start, channel, n, vel) = open.remove(idx);
                out.push(VizNote {
                    start,
                    end: ev.tick.max(start + 1),
                    note: n,
                    velocity: vel,
                    channel,
                });
            }
            open.push((ev.tick, ch, note, ev.velocity.max(1)));
        } else if let Some(idx) = open.iter().position(|(_, c, n, _)| *c == ch && *n == note) {
            let (start, channel, n, vel) = open.remove(idx);
            out.push(VizNote {
                start,
                end: ev.tick.max(start + 1),
                note: n,
                velocity: vel,
                channel,
            });
        }
    }
    for (start, channel, note, vel) in open {
        out.push(VizNote {
            start,
            end: length.max(start + 1),
            note,
            velocity: vel,
            channel,
        });
    }
    out.sort_by_key(|n| (n.start, n.note, n.channel));
    out
}

pub fn pitched_notes(notes: &[VizNote]) -> impl Iterator<Item = &VizNote> {
    notes.iter().filter(|n| !n.is_drum())
}

pub fn drum_pitches(notes: &[VizNote]) -> Vec<u8> {
    let mut out: Vec<u8> = notes
        .iter()
        .filter(|n| n.is_drum())
        .map(|n| n.note)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Full pitched range (no 3-octave clamp) so the keyboard matches the file.
pub fn pitched_range(notes: &[VizNote]) -> (u8, u8) {
    let mut lo = 127u8;
    let mut hi = 0u8;
    for n in pitched_notes(notes) {
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
    } else {
        let a = lo.saturating_sub(1);
        let b = hi.saturating_add(1).min(127);
        (a, b.max(a + 1))
    }
}

/// Pitch-class rainbow (C=red … B=violet). Drums stay amber.
pub fn note_color(note: &VizNote, active: bool) -> u32 {
    if note.is_drum() {
        let v = if active { 1.0 } else { 0.70 };
        return kaoss_viz::hsv_color(0.10, 0.82, v);
    }
    let pc = (note.note % 12) as f32;
    let h = pc / 12.0;
    let vel = (note.velocity as f32 / 127.0).clamp(0.25, 1.0);
    let v = if active {
        0.72 + 0.28 * vel
    } else {
        0.42 + 0.34 * vel
    };
    let s = if active { 0.88 } else { 0.72 };
    kaoss_viz::hsv_color(h, s, v)
}

pub fn key_is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

pub fn drum_label(note: u8) -> &'static str {
    match note {
        35 | 36 => "K",
        37 | 38 => "S",
        39 | 42 | 44 => "H",
        46 | 54 => "O",
        41 | 43 | 45 => "T",
        47 | 48 | 50 => "M",
        49 | 57 => "C",
        51 | 59 => "R",
        _ => "D",
    }
}

/// How many white keys sit at or below `note` (C-1 = 0).
pub fn white_index(note: u8) -> i32 {
    const WHITE: [i32; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
    let n = note as i32;
    (n / 12) * 7 + WHITE[(n % 12) as usize]
}

pub fn first_white_on_or_below(note: u8) -> u8 {
    let mut n = note;
    while key_is_black(n) && n > 0 {
        n -= 1;
    }
    n
}

pub fn first_white_on_or_above(note: u8) -> u8 {
    let mut n = note;
    while key_is_black(n) && n < 127 {
        n += 1;
    }
    n
}

/// Beats of upcoming score above the hit line.
pub const FALL_AHEAD_BEATS: f32 = 5.0;

pub fn fall_ahead_ticks() -> u32 {
    ((PPQ as f32) * FALL_AHEAD_BEATS).round() as u32
}

/// Y of a tick: the hit line is "now"; later ticks sit higher.
pub fn tick_y(tick: i64, playhead: i64, hit_y: f32, pixels_per_tick: f32) -> f32 {
    hit_y - (tick - playhead) as f32 * pixels_per_tick
}

pub fn fall_pixels_per_tick(fall: Rect) -> f32 {
    fall.h as f32 / fall_ahead_ticks().max(1) as f32
}

/// Visible falling-notes span as `[start, end)` ticks. A second pair is the
/// wrap-around when LOOP is on and the window crosses the file end.
pub fn overview_window(playhead: u32, length: u32, looping: bool) -> [(u32, u32); 2] {
    let length = length.max(1);
    let ahead = fall_ahead_ticks();
    let start = playhead.min(length);
    let end = playhead.saturating_add(ahead);
    if end <= length {
        [(start, end), (0, 0)]
    } else if looping {
        [(start, length), (0, (end - length).min(length))]
    } else {
        [(start, length), (0, 0)]
    }
}

pub fn overview_span_x(start: u32, end: u32, length: u32, bar: Rect) -> (f32, f32) {
    let w = bar.w.max(1) as f32;
    let length = length.max(1) as f32;
    let x0 = bar.x as f32 + (start as f32 / length) * w;
    let x1 = bar.x as f32 + (end.min(length as u32) as f32 / length) * w;
    let x1 = x1.max(x0 + 4.0).min(bar.x as f32 + w);
    (x0, x1)
}

/// White-key column for a MIDI note inside `piano`.
pub fn white_key_rect(note: u8, lo: u8, hi: u8, piano: Rect) -> Rect {
    let left = first_white_on_or_below(lo);
    let right = first_white_on_or_above(hi);
    let i0 = white_index(left);
    let i1 = white_index(right);
    let count = (i1 - i0 + 1).max(1);
    let w = piano.w as f32 / count as f32;
    let idx = (white_index(first_white_on_or_below(note)) - i0).max(0);
    Rect {
        x: piano.x + (idx as f32 * w).round() as i32,
        y: piano.y,
        w: w.round().max(2.0) as i32,
        h: piano.h,
    }
}

/// Black-key overlay (narrower, shorter) for a sharp / flat.
pub fn black_key_rect(note: u8, lo: u8, hi: u8, piano: Rect) -> Option<Rect> {
    if !key_is_black(note) || note < lo || note > hi {
        return None;
    }
    let below = white_key_rect(note - 1, lo, hi, piano);
    let w = (below.w as f32 * 0.62).round().max(3.0) as i32;
    Some(Rect {
        x: below.x + below.w - w / 2,
        y: piano.y,
        w,
        h: (piano.h as f32 * 0.58).round() as i32,
    })
}

pub fn drum_column(index: usize, count: usize, drums: Rect) -> Rect {
    let n = count.max(1) as i32;
    let w = (drums.w / n).max(8);
    Rect {
        x: drums.x + index as i32 * w,
        y: drums.y,
        w,
        h: drums.h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_on_off_and_hanging_note() {
        let events = [
            WireClipEvent::midi(0, true, 0, 60, 100),
            WireClipEvent::midi(480, false, 0, 60, 0),
            WireClipEvent::midi(960, true, 0, 64, 80),
        ];
        let notes = pair_notes(&events, 1920);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].start, 0);
        assert_eq!(notes[0].end, 480);
        assert_eq!(notes[0].note, 60);
        assert_eq!(notes[1].start, 960);
        assert_eq!(notes[1].end, 1920);
        assert_eq!(notes[1].note, 64);
    }

    #[test]
    fn retrigger_closes_previous() {
        let events = [
            WireClipEvent::midi(0, true, 0, 60, 90),
            WireClipEvent::midi(120, true, 0, 60, 90),
            WireClipEvent::midi(240, false, 0, 60, 0),
        ];
        let notes = pair_notes(&events, 480);
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].end, 120);
        assert_eq!(notes[1].start, 120);
        assert_eq!(notes[1].end, 240);
    }

    #[test]
    fn pitched_range_ignores_drum_extremes() {
        let notes = [
            VizNote {
                start: 0,
                end: 10,
                note: 36,
                velocity: 100,
                channel: 9,
            },
            VizNote {
                start: 0,
                end: 10,
                note: 60,
                velocity: 100,
                channel: 0,
            },
            VizNote {
                start: 0,
                end: 10,
                note: 64,
                velocity: 100,
                channel: 0,
            },
        ];
        let (lo, hi) = pitched_range(&notes);
        assert!(lo <= 60 && 64 <= hi);
        assert!(lo > 36 || hi < 80, "drums must not yank the keyboard to kick");
        assert_eq!(drum_pitches(&notes), vec![36]);
    }

    #[test]
    fn later_ticks_sit_above_the_hit_line() {
        let y = tick_y(960, 0, 400.0, 0.1);
        assert!(y < 400.0);
    }

    #[test]
    fn overview_window_covers_the_falling_span() {
        let length = 20_000;
        let [(a0, a1), wrap] = overview_window(1_000, length, false);
        assert_eq!(a0, 1_000);
        assert_eq!(a1, 1_000 + fall_ahead_ticks());
        assert_eq!(wrap, (0, 0));
        let [(b0, b1), (w0, w1)] = overview_window(length - 100, length, true);
        assert_eq!(b0, length - 100);
        assert_eq!(b1, length);
        assert_eq!(w0, 0);
        assert!(w1 > 0);
    }
}
