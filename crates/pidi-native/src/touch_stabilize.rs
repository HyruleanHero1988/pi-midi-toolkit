//! Absorb spurious `tracking_id → -1` lifts from FT5x06 / edt-ft5x06 panels.
//!
//! The chip often clears the contact while the finger is still down (especially
//! when held still). Real taps must still lift quickly; only longer holds get a
//! longer grace when position has been quiet.

use std::time::{Duration, Instant};

use crate::input::TouchEvent;

/// Quick rebind while the finger is moving or recently moved.
const REBIND_GRACE: Duration = Duration::from_millis(180);
/// Still finger: kernel goes quiet for hundreds of ms before reacquire.
const STILL_GRACE: Duration = Duration::from_millis(900);
/// Shorter than this → immediate up (no sticky tap).
const MIN_HOLD_FOR_GRACE: Duration = Duration::from_millis(120);
/// Hold long enough with no move → use the longer still window.
const STILL_HOLD_MIN: Duration = Duration::from_millis(400);
const STILL_QUIET: Duration = Duration::from_millis(280);
pub const REBIND_DIST_PX: i32 = 48;

#[derive(Clone, Copy, Debug, Default)]
struct Contact {
    x: i32,
    y: i32,
    held_since: Option<Instant>,
    last_move: Option<Instant>,
    pending_up: Option<Instant>,
}

#[derive(Default)]
pub struct TouchFilter {
    by_id: [Contact; 16],
}

impl TouchFilter {
    pub fn ingest(&mut self, incoming: Vec<TouchEvent>, now: Instant) -> Vec<TouchEvent> {
        let mut out = Vec::with_capacity(incoming.len());
        for ev in incoming {
            match ev {
                TouchEvent::Down { id, x, y } => {
                    if let Some(rebound) = self.take_pending_near(id, x, y, now) {
                        out.push(TouchEvent::Move {
                            id: rebound,
                            x,
                            y,
                        });
                        self.by_id[slot(rebound)].note_move(now, x, y);
                        continue;
                    }
                    let s = slot(id);
                    self.by_id[s].note_down(now, x, y);
                    out.push(TouchEvent::Down { id, x, y });
                }
                TouchEvent::Move { id, x, y } => {
                    self.by_id[slot(id)].note_move(now, x, y);
                    self.by_id[slot(id)].pending_up = None;
                    out.push(TouchEvent::Move { id, x, y });
                }
                TouchEvent::Up { id, x, y } => {
                    let s = slot(id);
                    let c = &mut self.by_id[s];
                    let pos = position(x, y, c.x, c.y);
                    c.x = pos.0;
                    c.y = pos.1;
                    match grace_deadline(c, now) {
                        None => {
                            c.reset();
                            out.push(TouchEvent::Up { id, x, y });
                        }
                        Some(deadline) => {
                            c.pending_up = Some(deadline);
                        }
                    }
                }
            }
        }
        out
    }

    pub fn expire(&mut self, now: Instant) -> Vec<TouchEvent> {
        let mut out = Vec::new();
        for id in 0..self.by_id.len() as i32 {
            let c = &mut self.by_id[id as usize];
            let Some(deadline) = c.pending_up else {
                continue;
            };
            if now < deadline {
                continue;
            }
            out.push(TouchEvent::Up {
                id,
                x: Some(c.x),
                y: Some(c.y),
            });
            c.reset();
        }
        out
    }

  fn take_pending_near(&mut self, new_id: i32, x: i32, y: i32, now: Instant) -> Option<i32> {
        let mut best: Option<(i32, i64)> = None; // (id, dist2)
        for id in 0..self.by_id.len() as i32 {
            if id == new_id {
                continue;
            }
            let c = &self.by_id[id as usize];
            let Some(deadline) = c.pending_up else {
                continue;
            };
            if now > deadline {
                continue;
            }
            let d2 = dist2(c.x, c.y, x, y);
            if d2 > (REBIND_DIST_PX as i64) * (REBIND_DIST_PX as i64) {
                continue;
            }
            if best.map(|(_, bd)| d2 < bd).unwrap_or(true) {
                best = Some((id, d2));
            }
        }
        if let Some((id, _)) = best {
            self.by_id[id as usize].pending_up = None;
            self.by_id[id as usize].note_move(now, x, y);
            // New Down on a different SDL FingerId should not stick around.
            self.by_id[slot(new_id)].reset();
            return Some(id);
        }
        // Same slot rebind before we surfaced the Up (evdev keeps id stable).
        let c = &mut self.by_id[slot(new_id)];
        if c.pending_up.take().is_some() {
            c.note_move(now, x, y);
            return Some(new_id);
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SlotStabilizer {
    held_since: Option<Instant>,
    last_move: Option<Instant>,
    pending_up: Option<Instant>,
    x: i32,
    y: i32,
}

impl SlotStabilizer {
    pub fn note_down(&mut self, now: Instant, x: i32, y: i32) {
        if self.held_since.is_none() {
            self.held_since = Some(now);
        }
        self.last_move = Some(now);
        self.x = x;
        self.y = y;
        self.pending_up = None;
    }

    pub fn note_move(&mut self, now: Instant, x: i32, y: i32) {
        if self.held_since.is_none() {
            self.held_since = Some(now);
        }
        self.last_move = Some(now);
        self.x = x;
        self.y = y;
        self.pending_up = None;
    }

    /// `true` when the lift should be deferred (no `TouchEvent::Up` yet).
    pub fn defer_lift(&mut self, now: Instant) -> bool {
        if let Some(deadline) = grace_deadline(self, now) {
            self.pending_up = Some(deadline);
            true
        } else {
            false
        }
    }

    pub fn absorb_rebind(&mut self) {
        self.pending_up = None;
    }

    pub fn pending_up(&self) -> Option<Instant> {
        self.pending_up
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn pos(&self) -> (i32, i32) {
        (self.x, self.y)
    }

    pub fn expire(&mut self, now: Instant) -> bool {
        let Some(deadline) = self.pending_up else {
            return false;
        };
        if now < deadline {
            return false;
        }
        self.pending_up = None;
        true
    }
}

impl Contact {
    fn note_down(&mut self, now: Instant, x: i32, y: i32) {
        self.held_since = Some(now);
        self.last_move = Some(now);
        self.x = x;
        self.y = y;
        self.pending_up = None;
    }

    fn note_move(&mut self, now: Instant, x: i32, y: i32) {
        if self.held_since.is_none() {
            self.held_since = Some(now);
        }
        self.last_move = Some(now);
        self.x = x;
        self.y = y;
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

fn grace_deadline(
    c: &impl HasGraceFields,
    now: Instant,
) -> Option<Instant> {
    let held_since = c.held_since()?;
    let held = now.saturating_duration_since(held_since);
    if held < MIN_HOLD_FOR_GRACE {
        return None;
    }
    let quiet = c
        .last_move()
        .map(|t| now.saturating_duration_since(t) >= STILL_QUIET)
        .unwrap_or(false);
    let grace = if quiet && held >= STILL_HOLD_MIN {
        STILL_GRACE
    } else {
        REBIND_GRACE
    };
    Some(now + grace)
}

trait HasGraceFields {
    fn held_since(&self) -> Option<Instant>;
    fn last_move(&self) -> Option<Instant>;
}

impl HasGraceFields for Contact {
    fn held_since(&self) -> Option<Instant> {
        self.held_since
    }
    fn last_move(&self) -> Option<Instant> {
        self.last_move
    }
}

impl HasGraceFields for SlotStabilizer {
    fn held_since(&self) -> Option<Instant> {
        self.held_since
    }
    fn last_move(&self) -> Option<Instant> {
        self.last_move
    }
}

fn slot(id: i32) -> usize {
    id.clamp(0, 15) as usize
}

fn position(x: Option<i32>, y: Option<i32>, fx: i32, fy: i32) -> (i32, i32) {
    (x.unwrap_or(fx), y.unwrap_or(fy))
}

fn dist2(ax: i32, ay: i32, bx: i32, by: i32) -> i64 {
    let dx = (ax - bx) as i64;
    let dy = (ay - by) as i64;
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_tap_lifts_immediately() {
        let mut f = TouchFilter::default();
        let t0 = Instant::now();
        let mut out = f.ingest(
            vec![TouchEvent::Down {
                id: 1,
                x: 100,
                y: 100,
            }],
            t0,
        );
        assert_eq!(out.len(), 1);
        out = f.ingest(
            vec![TouchEvent::Up {
                id: 1,
                x: Some(100),
                y: Some(100),
            }],
            t0 + Duration::from_millis(50),
        );
        assert!(matches!(out.as_slice(), [TouchEvent::Up { id: 1, .. }]));
    }

    #[test]
    fn held_still_defers_then_expires() {
        let mut f = TouchFilter::default();
        let t0 = Instant::now();
        f.ingest(
            vec![TouchEvent::Down {
                id: 0,
                x: 200,
                y: 200,
            }],
            t0,
        );
        let mut out = f.ingest(
            vec![TouchEvent::Up {
                id: 0,
                x: Some(200),
                y: Some(200),
            }],
            t0 + Duration::from_millis(600),
        );
        assert!(out.is_empty(), "still hold should defer: {out:?}");
        out = f.expire(t0 + Duration::from_millis(700));
        assert!(out.is_empty());
        out = f.expire(t0 + Duration::from_millis(1600));
        assert!(matches!(out.as_slice(), [TouchEvent::Up { id: 0, .. }]));
    }

    #[test]
    fn sdl_rebind_absorbs_as_move() {
        let mut f = TouchFilter::default();
        let t0 = Instant::now();
        f.ingest(
            vec![TouchEvent::Down {
                id: 7,
                x: 300,
                y: 220,
            }],
            t0,
        );
        f.ingest(
            vec![TouchEvent::Move {
                id: 7,
                x: 310,
                y: 225,
            }],
            t0 + Duration::from_millis(200),
        );
        let out = f.ingest(
            vec![
                TouchEvent::Up {
                    id: 7,
                    x: Some(310),
                    y: Some(225),
                },
                TouchEvent::Down {
                    id: 99,
                    x: 312,
                    y: 226,
                },
            ],
            t0 + Duration::from_millis(500),
        );
        assert!(
            matches!(
                out.as_slice(),
                [TouchEvent::Move { id: 7, x: 312, y: 226 }]
            ),
            "rebind should keep original finger id: {out:?}"
        );
    }
}