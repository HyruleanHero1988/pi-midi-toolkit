//! Instrument-surface state. Rendering and IPC consume this; they do not own notes.

use crate::client::Outbox;
use crate::layout::{Hit, Layout, Surface};
use jambox_protocol::{RepeatDivision, RepeatPhase, StatusReply, TouchPhase};

pub const LED_COLS: usize = 12;
pub const LED_ROWS: usize = 7;
pub const KICK_NOTE: u8 = 36;
pub const DRUM_CHANNEL: u8 = 9;
pub const MAX_FINGERS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatDivisionChoice {
    Quarter,
    Eighth,
    EighthTriplet,
    Sixteenth,
}

impl RepeatDivisionChoice {
    pub fn from_index(index: usize) -> Self {
        match index % 4 {
            1 => Self::Eighth,
            2 => Self::EighthTriplet,
            3 => Self::Sixteenth,
            _ => Self::Quarter,
        }
    }

    pub fn as_wire(self) -> RepeatDivision {
        match self {
            Self::Quarter => RepeatDivision::Quarter,
            Self::Eighth => RepeatDivision::Eighth,
            Self::EighthTriplet => RepeatDivision::EighthTriplet,
            Self::Sixteenth => RepeatDivision::Sixteenth,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::EighthTriplet => "1/8T",
            Self::Sixteenth => "1/16",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Finger {
    active: bool,
    id: i32,
    gesture: u32,
    x: f32,
    y: f32,
    px: i32,
    py: i32,
    surface: Surface,
}

impl Finger {
    const fn silent() -> Self {
        Self {
            active: false,
            id: -1,
            gesture: 0,
            x: 0.0,
            y: 0.0,
            px: 0,
            py: 0,
            surface: Surface::Kaoss,
        }
    }
}

pub struct NativeModel {
    pub layout: Layout,
    pub division: RepeatDivisionChoice,
    pub status: StatusReply,
    pub fps: f32,
    pub connected: bool,
    pub frame: u64,
    fingers: [Finger; MAX_FINGERS],
    next_gesture: u32,
    cells: [[u32; LED_COLS]; LED_ROWS],
}

impl Default for NativeModel {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeModel {
    pub fn new() -> Self {
        Self {
            layout: Layout::new(),
            division: RepeatDivisionChoice::Quarter,
            status: StatusReply::default(),
            fps: 0.0,
            connected: false,
            frame: 0,
            fingers: [Finger::silent(); MAX_FINGERS],
            next_gesture: 1,
            cells: [[0; LED_COLS]; LED_ROWS],
        }
    }

    pub fn active_fingers(&self) -> usize {
        self.fingers.iter().filter(|f| f.active).count()
    }

    pub fn kaoss_finger(&self) -> Option<(f32, f32)> {
        self.fingers
            .iter()
            .find(|f| f.active && f.surface == Surface::Kaoss)
            .map(|f| (f.x, f.y))
    }

    pub fn cell(&self, col: usize, row: usize) -> u32 {
        self.cells[row][col]
    }

    pub fn tick(&mut self, dt: f32) {
        self.frame = self.frame.wrapping_add(1);
        if dt > 0.0001 {
            let inst = 1.0 / dt;
            self.fps = if self.fps <= 0.1 {
                inst
            } else {
                self.fps * 0.9 + inst * 0.1
            };
        }
        self.paint_cells();
    }

    pub fn finger_down(&mut self, id: i32, px: i32, py: i32, outbox: &mut Outbox) {
        if self.fingers.iter().any(|f| f.active && f.id == id) {
            self.finger_move(id, px, py, outbox);
            return;
        }
        let slot = match self.fingers.iter().position(|f| !f.active) {
            Some(s) => s,
            None => return,
        };
        let gesture = self.next_gesture;
        self.next_gesture = self.next_gesture.wrapping_add(1).max(1);
        match self.layout.hit(px, py) {
            Hit::Kaoss { x, y } => {
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x,
                    y,
                    px,
                    py,
                    surface: Surface::Kaoss,
                };
                outbox.touch(gesture, TouchPhase::Down, x, y);
            }
            Hit::Drum { note, .. } => {
                let repeat = note == KICK_NOTE;
                self.fingers[slot] = Finger {
                    active: true,
                    id,
                    gesture,
                    x: 0.0,
                    y: 0.0,
                    px,
                    py,
                    surface: Surface::Drum { note, repeat },
                };
                if repeat {
                    outbox.repeat(
                        gesture,
                        RepeatPhase::Down,
                        note,
                        DRUM_CHANNEL,
                        110,
                        self.division.as_wire(),
                    );
                } else {
                    outbox.note_on(DRUM_CHANNEL, note, 110);
                }
            }
            Hit::Division(index) => {
                self.division = RepeatDivisionChoice::from_index(index);
            }
            Hit::None => {}
        }
    }

    pub fn finger_move(&mut self, id: i32, px: i32, py: i32, outbox: &mut Outbox) {
        let Some(slot) = self.fingers.iter().position(|f| f.active && f.id == id) else {
            return;
        };
        self.fingers[slot].px = px;
        self.fingers[slot].py = py;
        if self.fingers[slot].surface == Surface::Kaoss {
            let (x, y) = self.layout.kaoss.pad_xy(px, py);
            self.fingers[slot].x = x;
            self.fingers[slot].y = y;
            outbox.touch(self.fingers[slot].gesture, TouchPhase::Move, x, y);
        }
        // Drum/repeat capture-until-lift: sliding off the pad does not re-hit.
    }

    pub fn finger_up(&mut self, id: i32, outbox: &mut Outbox) {
        let Some(slot) = self.fingers.iter().position(|f| f.active && f.id == id) else {
            return;
        };
        let finger = self.fingers[slot];
        self.fingers[slot] = Finger::silent();
        match finger.surface {
            Surface::Kaoss => outbox.touch(finger.gesture, TouchPhase::Up, finger.x, finger.y),
            Surface::Drum { note, repeat } => {
                if repeat {
                    outbox.repeat(
                        finger.gesture,
                        RepeatPhase::Up,
                        note,
                        DRUM_CHANNEL,
                        110,
                        self.division.as_wire(),
                    );
                }
            }
        }
    }

    pub fn cancel_all(&mut self, outbox: &mut Outbox) {
        let active: Vec<i32> = self
            .fingers
            .iter()
            .filter(|f| f.active)
            .map(|f| f.id)
            .collect();
        for id in active {
            self.finger_up(id, outbox);
        }
    }

    fn paint_cells(&mut self) {
        let t = self.frame as f32 / 60.0;
        let finger = self.kaoss_finger();
        for row in 0..LED_ROWS {
            for col in 0..LED_COLS {
                self.cells[row][col] = pad_led_rgb(col, row, t, finger);
            }
        }
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> u32 {
    let h = h.rem_euclid(1.0);
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let sector = h * 6.0;
    let i = sector as i32;
    let f = sector - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    ((r * 255.0) as u32) << 16 | ((g * 255.0) as u32) << 8 | (b * 255.0) as u32
}

pub fn pad_led_rgb(col: usize, row: usize, t: f32, finger: Option<(f32, f32)>) -> u32 {
    let lx = col as f32 / (LED_COLS - 1) as f32;
    let ly = row as f32 / (LED_ROWS - 1) as f32;
    let wave = 0.5 + 0.5 * (t * 1.6 + col as f32 * 0.45 + row as f32 * 0.38).sin();
    let mut hue = (lx * 0.70 + t * 0.035).rem_euclid(1.0);
    let mut sat = 0.82;
    let mut val = 0.045 + 0.09 * wave;
    if let Some((fx, fy)) = finger {
        let dist = ((lx - fx).hypot(ly - fy)).max(0.0);
        let glow = (1.0 - dist / 0.40).max(0.0).powf(1.45);
        val = (val + glow * 0.92).min(1.0);
        sat = (0.55 + glow * 0.45).min(1.0);
        hue = (hue * (1.0 - glow * 0.55) + (fx * 0.70) * glow).rem_euclid(1.0);
    }
    hsv_to_rgb(hue, sat, val)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Outbox;
    use jambox_protocol::Request;

    #[test]
    fn kick_hold_is_a_repeat_edge_not_a_fifo_of_moves() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let cell = model.layout.drum_cell(0);
        model.finger_down(1, cell.x + 4, cell.y + 4, &mut out);
        for i in 0..50 {
            model.finger_move(1, cell.x + 4 + i, cell.y + 4, &mut out);
        }
        model.finger_up(1, &mut out);
        let batch = out.take();
        assert!(matches!(
            batch[0],
            Request::Repeat {
                phase: RepeatPhase::Down,
                note: 36,
                ..
            }
        ));
        assert!(matches!(
            batch.last(),
            Some(Request::Repeat {
                phase: RepeatPhase::Up,
                ..
            })
        ));
        assert!(batch
            .iter()
            .all(|r| !matches!(r, Request::Touch { .. })));
    }

    #[test]
    fn kaoss_moves_coalesce_by_gesture() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let k = model.layout.kaoss;
        model.finger_down(2, k.x + 10, k.y + 10, &mut out);
        for i in 0..80 {
            model.finger_move(2, k.x + 10 + i, k.y + 10, &mut out);
        }
        let batch = out.take();
        let moves = batch
            .iter()
            .filter(|r| matches!(r, Request::Touch { phase: TouchPhase::Move, .. }))
            .count();
        assert_eq!(moves, 1, "stale XY must not queue behind the lift");
        assert!(matches!(
            batch[0],
            Request::Touch {
                phase: TouchPhase::Down,
                ..
            }
        ));
    }

    #[test]
    fn a_snare_can_fire_while_kick_repeats() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let kick = model.layout.drum_cell(0);
        let snare = model.layout.drum_cell(1);
        model.finger_down(1, kick.x + 4, kick.y + 4, &mut out);
        model.finger_down(2, snare.x + 4, snare.y + 4, &mut out);
        assert_eq!(model.active_fingers(), 2);
        let batch = out.take();
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::Repeat {
                note: 36,
                phase: RepeatPhase::Down,
                ..
            }
        )));
        assert!(batch.iter().any(|r| matches!(
            r,
            Request::NoteOn {
                note: 37,
                channel: 9,
                ..
            }
        )));
    }

    #[test]
    fn five_contacts_are_tracked() {
        let mut model = NativeModel::new();
        let mut out = Outbox::new();
        let k = model.layout.kaoss;
        for i in 0..5 {
            model.finger_down(i, k.x + 20 + i * 40, k.y + 40, &mut out);
        }
        assert_eq!(model.active_fingers(), 5);
        model.finger_down(99, k.x + 100, k.y + 100, &mut out);
        assert_eq!(model.active_fingers(), 5);
    }
}
