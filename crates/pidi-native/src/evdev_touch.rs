//! Linux evdev multitouch decoder (type-A and type-B).
//!
//! The appliance path (`--touch evdev`) reads `/dev/input/event*` directly.
//! Type-B (ABS_MT_SLOT) is what GT911 should expose. Several Waveshare overlays
//! still emit type-A (contacts packed between SYN_MT_REPORT). The old decoder
//! only kept `current` slot 0, so a second finger overwrote the first and the
//! Kaoss pad looked monophonic even after per-finger GLOW state landed.

use crate::input::TouchEvent;

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_ABS: u16 = 3;
pub const SYN_REPORT: u16 = 0;
pub const SYN_MT_REPORT: u16 = 2;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const BTN_TOUCH: u16 = 0x14a;

const MAX_SLOTS: usize = 10;

#[derive(Clone, Copy)]
struct Slot {
    tracking: i32,
    last_id: i32,
    x: i32,
    y: i32,
    dirty: bool,
    was_active: bool,
}

impl Slot {
    const fn new() -> Self {
        Self {
            tracking: -1,
            last_id: -1,
            x: 0,
            y: 0,
            dirty: false,
            was_active: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RawEvent {
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

impl RawEvent {
    pub fn abs(code: u16, value: i32) -> Self {
        Self {
            type_: EV_ABS,
            code,
            value,
        }
    }

    pub fn syn(code: u16) -> Self {
        Self {
            type_: EV_SYN,
            code,
            value: 0,
        }
    }

    pub fn key(code: u16, value: i32) -> Self {
        Self {
            type_: EV_KEY,
            code,
            value,
        }
    }
}

/// Incremental parser for one capacitive panel.
pub struct EvdevDecoder {
    slots: [Slot; MAX_SLOTS],
    current: usize,
    type_b: bool,
    type_a: Slot,
    type_a_frame: Vec<(i32, i32, i32)>,
    type_a_prev: Vec<(i32, i32, i32)>,
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    screen_w: i32,
    screen_h: i32,
}

impl EvdevDecoder {
    pub fn new() -> Self {
        Self {
            slots: [Slot::new(); MAX_SLOTS],
            current: 0,
            type_b: false,
            type_a: Slot::new(),
            type_a_frame: Vec::with_capacity(MAX_SLOTS),
            type_a_prev: Vec::with_capacity(MAX_SLOTS),
            min_x: 0,
            max_x: 800,
            min_y: 0,
            max_y: 480,
            screen_w: 800,
            screen_h: 480,
        }
    }

    pub fn set_abs_range(&mut self, min_x: i32, max_x: i32, min_y: i32, max_y: i32) {
        self.min_x = min_x;
        self.max_x = max_x;
        self.min_y = min_y;
        self.max_y = max_y;
    }

    pub fn set_screen(&mut self, w: i32, h: i32) {
        self.screen_w = w.max(1);
        self.screen_h = h.max(1);
    }

    pub fn feed(&mut self, ev: RawEvent, out: &mut Vec<TouchEvent>) {
        match ev.type_ {
            EV_ABS => match ev.code {
                ABS_MT_SLOT => {
                    self.type_b = true;
                    self.current = (ev.value as usize).min(self.slots.len() - 1);
                }
                ABS_MT_TRACKING_ID => {
                    if self.type_b {
                        self.slots[self.current].tracking = ev.value;
                        self.slots[self.current].dirty = true;
                    } else {
                        self.type_a.tracking = ev.value;
                    }
                }
                ABS_MT_POSITION_X => {
                    if self.type_b {
                        self.slots[self.current].x = ev.value;
                        self.slots[self.current].dirty = true;
                    } else {
                        self.type_a.x = ev.value;
                    }
                }
                ABS_MT_POSITION_Y => {
                    if self.type_b {
                        self.slots[self.current].y = ev.value;
                        self.slots[self.current].dirty = true;
                    } else {
                        self.type_a.y = ev.value;
                    }
                }
                _ => {}
            },
            EV_KEY => {
                if ev.code == BTN_TOUCH && ev.value == 0 && !self.type_b {
                    self.type_a.tracking = -1;
                    self.type_a_frame.clear();
                }
            }
            EV_SYN => {
                if ev.code == SYN_MT_REPORT && !self.type_b {
                    self.push_type_a_contact();
                } else if ev.code == SYN_REPORT {
                    if self.type_b {
                        self.flush_type_b(out);
                    } else {
                        self.push_type_a_contact();
                        self.flush_type_a(out);
                    }
                }
            }
            _ => {}
        }
    }

    fn push_type_a_contact(&mut self) {
        if self.type_a.tracking >= 0 {
            self.type_a_frame
                .push((self.type_a.tracking, self.type_a.x, self.type_a.y));
            self.type_a.tracking = -1;
        }
    }

    fn flush_type_b(&mut self, out: &mut Vec<TouchEvent>) {
        for slot in &mut self.slots {
            if !slot.dirty {
                continue;
            }
            slot.dirty = false;
            let x = map(slot.x, self.min_x, self.max_x, self.screen_w);
            let y = map(slot.y, self.min_y, self.max_y, self.screen_h);
            if slot.tracking < 0 {
                if slot.was_active && slot.last_id >= 0 {
                    out.push(TouchEvent::Up { id: slot.last_id });
                }
                slot.was_active = false;
                slot.last_id = -1;
            } else {
                slot.last_id = slot.tracking;
                if !slot.was_active {
                    out.push(TouchEvent::Down {
                        id: slot.last_id,
                        x,
                        y,
                    });
                    slot.was_active = true;
                } else {
                    out.push(TouchEvent::Move {
                        id: slot.last_id,
                        x,
                        y,
                    });
                }
            }
        }
    }

    fn flush_type_a(&mut self, out: &mut Vec<TouchEvent>) {
        let frame = std::mem::take(&mut self.type_a_frame);
        for &(id, _, _) in &self.type_a_prev {
            if !frame.iter().any(|(fid, _, _)| *fid == id) {
                out.push(TouchEvent::Up { id });
            }
        }
        for &(id, x, y) in &frame {
            let x = map(x, self.min_x, self.max_x, self.screen_w);
            let y = map(y, self.min_y, self.max_y, self.screen_h);
            if self.type_a_prev.iter().any(|(fid, _, _)| *fid == id) {
                out.push(TouchEvent::Move { id, x, y });
            } else {
                out.push(TouchEvent::Down { id, x, y });
            }
        }
        self.type_a_prev = frame;
    }
}

impl Default for EvdevDecoder {
    fn default() -> Self {
        Self::new()
    }
}

fn map(v: i32, min: i32, max: i32, screen: i32) -> i32 {
    if max <= min {
        return v.clamp(0, screen - 1);
    }
    ((v - min) as i64 * (screen as i64 - 1) / (max - min) as i64).clamp(0, screen as i64 - 1)
        as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn downs(out: &[TouchEvent]) -> Vec<(i32, i32, i32)> {
        out.iter()
            .filter_map(|e| match *e {
                TouchEvent::Down { id, x, y } => Some((id, x, y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn type_b_two_slots_are_independent() {
        let mut dec = EvdevDecoder::new();
        let mut out = Vec::new();
        dec.feed(RawEvent::abs(ABS_MT_SLOT, 0), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_TRACKING_ID, 5), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_X, 80), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_Y, 120), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_SLOT, 1), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_TRACKING_ID, 6), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_X, 720), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_Y, 120), &mut out);
        dec.feed(RawEvent::syn(SYN_REPORT), &mut out);
        let d = downs(&out);
        assert_eq!(d.len(), 2, "type-B must emit both contacts: {out:?}");
        assert_eq!(d[0].0, 5);
        assert_eq!(d[1].0, 6);
        assert!(d[0].1 < 200, "left contact should stay left, got {}", d[0].1);
        assert!(d[1].1 > 600, "right contact should stay right, got {}", d[1].1);
    }

    #[test]
    fn type_a_two_contacts_are_independent() {
        let mut dec = EvdevDecoder::new();
        let mut out = Vec::new();
        dec.feed(RawEvent::abs(ABS_MT_TRACKING_ID, 10), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_X, 80), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_Y, 200), &mut out);
        dec.feed(RawEvent::syn(SYN_MT_REPORT), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_TRACKING_ID, 11), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_X, 720), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_Y, 200), &mut out);
        dec.feed(RawEvent::syn(SYN_MT_REPORT), &mut out);
        dec.feed(RawEvent::syn(SYN_REPORT), &mut out);
        let d = downs(&out);
        assert_eq!(
            d.len(),
            2,
            "type-A packed contacts must not collapse onto slot 0: {out:?}"
        );
        assert_eq!(d[0].0, 10);
        assert_eq!(d[1].0, 11);
        assert!(d[0].1 < 200);
        assert!(d[1].1 > 600);
    }

    #[test]
    fn type_a_single_contact_without_mt_report() {
        let mut dec = EvdevDecoder::new();
        let mut out = Vec::new();
        dec.feed(RawEvent::abs(ABS_MT_TRACKING_ID, 3), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_X, 400), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_Y, 240), &mut out);
        dec.feed(RawEvent::syn(SYN_REPORT), &mut out);
        assert_eq!(downs(&out).len(), 1);
        out.clear();
        dec.feed(RawEvent::syn(SYN_REPORT), &mut out);
        assert!(
            out.iter().any(|e| matches!(e, TouchEvent::Up { id: 3 })),
            "empty type-A frame should lift the contact: {out:?}"
        );
    }

    #[test]
    fn type_b_lift_uses_tracking_id() {
        let mut dec = EvdevDecoder::new();
        let mut out = Vec::new();
        dec.feed(RawEvent::abs(ABS_MT_SLOT, 0), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_TRACKING_ID, 9), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_X, 10), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_POSITION_Y, 10), &mut out);
        dec.feed(RawEvent::syn(SYN_REPORT), &mut out);
        out.clear();
        dec.feed(RawEvent::abs(ABS_MT_SLOT, 0), &mut out);
        dec.feed(RawEvent::abs(ABS_MT_TRACKING_ID, -1), &mut out);
        dec.feed(RawEvent::syn(SYN_REPORT), &mut out);
        assert!(matches!(out.as_slice(), [TouchEvent::Up { id: 9 }]));
    }
}
