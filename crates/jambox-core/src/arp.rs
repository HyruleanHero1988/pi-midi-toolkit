//! Key-relative arpeggiator: authored intervals, MPK-style walk orders, latch.
//!
//! The audio thread owns this. Incoming notes are the root (and latch retarget),
//! never a block chord. Pattern steps are relative semitones from that root.

use crate::transport::{Transport, PPQ};

pub const MAX_ARP_STEPS: usize = 16;
pub const MAX_ARP_POOL: usize = 64;
pub const MAX_ARP_HELD: usize = 8;
pub const MAX_ARP_EVENTS_PER_BLOCK: usize = 32;
/// Extra octaves stacked above the written pattern (MPK 0..=3).
pub const MAX_ARP_OCTAVES: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArpOrder {
    #[default]
    Up,
    Down,
    /// Lowest → highest → lowest, repeating the ends (MPK Inclusive, up first).
    InclUp,
    /// Highest → lowest → highest, repeating the ends.
    InclDown,
    /// Lowest → highest → lowest, ends played once (MPK Exclusive, up first).
    ExclUp,
    /// Highest → lowest → highest, ends played once.
    ExclDown,
    Random,
    /// Authored step order, then +1 octave, +2, … (MPK Order).
    Order,
}

impl ArpOrder {
    pub const ALL: [Self; 8] = [
        Self::Up,
        Self::Down,
        Self::InclUp,
        Self::InclDown,
        Self::ExclUp,
        Self::ExclDown,
        Self::Random,
        Self::Order,
    ];

    pub fn from_u8(value: u8) -> Self {
        Self::ALL
            .get(value as usize)
            .copied()
            .unwrap_or(Self::Up)
    }

    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Up => "UP",
            Self::Down => "DOWN",
            Self::InclUp => "INC+",
            Self::InclDown => "INC-",
            Self::ExclUp => "EXC+",
            Self::ExclDown => "EXC-",
            Self::Random => "RAND",
            Self::Order => "ORD",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "down" => Self::Down,
            "incl" | "inclusive" | "incl_up" | "in_up" | "in↑" | "inc+" => Self::InclUp,
            "incl_down" | "in_down" | "in↓" | "inc-" => Self::InclDown,
            "excl" | "exclusive" | "excl_up" | "ex_up" | "ex↑" | "exc+" => Self::ExclUp,
            "excl_down" | "ex_down" | "ex↓" | "exc-" => Self::ExclDown,
            "rand" | "random" => Self::Random,
            "order" | "ord" | "as_played" | "as_written" => Self::Order,
            _ => Self::Up,
        }
    }

    pub const fn wire(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::InclUp => "incl_up",
            Self::InclDown => "incl_down",
            Self::ExclUp => "excl_up",
            Self::ExclDown => "excl_down",
            Self::Random => "random",
            Self::Order => "order",
        }
    }

    fn starts_at_top(self) -> bool {
        matches!(self, Self::Down | Self::InclDown | Self::ExclDown)
    }

    fn bounces(self) -> bool {
        matches!(
            self,
            Self::InclUp | Self::InclDown | Self::ExclUp | Self::ExclDown
        )
    }

    fn inclusive(self) -> bool {
        matches!(self, Self::InclUp | Self::InclDown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArpDivision {
    Quarter,
    QuarterTriplet,
    Eighth,
    EighthTriplet,
    #[default]
    Sixteenth,
    SixteenthTriplet,
    ThirtySecond,
    ThirtySecondTriplet,
}

impl ArpDivision {
    pub const ALL: [Self; 8] = [
        Self::Quarter,
        Self::QuarterTriplet,
        Self::Eighth,
        Self::EighthTriplet,
        Self::Sixteenth,
        Self::SixteenthTriplet,
        Self::ThirtySecond,
        Self::ThirtySecondTriplet,
    ];

    pub fn from_u8(value: u8) -> Self {
        Self::ALL
            .get(value as usize)
            .copied()
            .unwrap_or(Self::Sixteenth)
    }

    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Quarter => "1/4",
            Self::QuarterTriplet => "1/4T",
            Self::Eighth => "1/8",
            Self::EighthTriplet => "1/8T",
            Self::Sixteenth => "1/16",
            Self::SixteenthTriplet => "1/16T",
            Self::ThirtySecond => "1/32",
            Self::ThirtySecondTriplet => "1/32T",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "quarter" | "1/4" => Self::Quarter,
            "quarter_triplet" | "1/4t" => Self::QuarterTriplet,
            "eighth" | "1/8" => Self::Eighth,
            "eighth_triplet" | "1/8t" => Self::EighthTriplet,
            "sixteenth_triplet" | "1/16t" => Self::SixteenthTriplet,
            "thirty_second" | "1/32" => Self::ThirtySecond,
            "thirty_second_triplet" | "1/32t" => Self::ThirtySecondTriplet,
            _ => Self::Sixteenth,
        }
    }

    pub const fn wire(self) -> &'static str {
        match self {
            Self::Quarter => "quarter",
            Self::QuarterTriplet => "quarter_triplet",
            Self::Eighth => "eighth",
            Self::EighthTriplet => "eighth_triplet",
            Self::Sixteenth => "sixteenth",
            Self::SixteenthTriplet => "sixteenth_triplet",
            Self::ThirtySecond => "thirty_second",
            Self::ThirtySecondTriplet => "thirty_second_triplet",
        }
    }

    pub const fn ticks(self) -> u64 {
        match self {
            Self::Quarter => PPQ as u64,
            Self::QuarterTriplet => PPQ as u64 * 2 / 3,
            Self::Eighth => PPQ as u64 / 2,
            Self::EighthTriplet => PPQ as u64 / 3,
            Self::Sixteenth => PPQ as u64 / 4,
            Self::SixteenthTriplet => PPQ as u64 / 6,
            Self::ThirtySecond => PPQ as u64 / 8,
            Self::ThirtySecondTriplet => PPQ as u64 / 12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArpEvent {
    pub frame: u32,
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
    pub on: bool,
}

#[derive(Debug, Clone, Copy)]
struct HeldKey {
    note: u8,
    velocity: u8,
}

/// Sample-clocked monophonic arp. Pattern is relative; keys only choose root.
pub struct Arpeggiator {
    enabled: bool,
    latch_armed: bool,
    latched: bool,
    order: ArpOrder,
    division: ArpDivision,
    octaves: u8,
    /// 1..=127; note length as a fraction of the step (127 = legato to next).
    gate: u8,
    channel: u8,
    steps: [i8; MAX_ARP_STEPS],
    step_len: u8,
    held: [HeldKey; MAX_ARP_HELD],
    held_len: u8,
    root: u8,
    velocity: u8,
    pool: [u8; MAX_ARP_POOL],
    pool_len: u8,
    cursor: u8,
    dir: i8,
    rng: u32,
    running: bool,
    sounding: Option<u8>,
    next_on_frame: u64,
    next_off_frame: Option<u64>,
}

impl Default for Arpeggiator {
    fn default() -> Self {
        Self::new()
    }
}

impl Arpeggiator {
    pub fn new() -> Self {
        let mut arp = Self {
            enabled: false,
            latch_armed: false,
            latched: false,
            order: ArpOrder::Up,
            division: ArpDivision::Sixteenth,
            octaves: 1,
            gate: 90,
            channel: 0,
            steps: [0; MAX_ARP_STEPS],
            step_len: 3,
            held: [HeldKey {
                note: 0,
                velocity: 0,
            }; MAX_ARP_HELD],
            held_len: 0,
            root: 60,
            velocity: 110,
            pool: [0; MAX_ARP_POOL],
            pool_len: 0,
            cursor: 0,
            dir: 1,
            rng: 0xA341_316C,
            running: false,
            sounding: None,
            next_on_frame: 0,
            next_off_frame: None,
        };
        arp.steps[0] = 0;
        arp.steps[1] = 4;
        arp.steps[2] = 7;
        arp.rebuild_pool();
        arp
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn latch_armed(&self) -> bool {
        self.latch_armed
    }

    pub fn latched(&self) -> bool {
        self.latched
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn root(&self) -> u8 {
        self.root
    }

    pub fn cursor(&self) -> u8 {
        self.cursor
    }

    pub fn order(&self) -> ArpOrder {
        self.order
    }

    pub fn division(&self) -> ArpDivision {
        self.division
    }

    pub fn set_enabled(&mut self, on: bool) -> Option<(u8, u8)> {
        self.enabled = on;
        if !on {
            self.stop()
        } else {
            None
        }
    }

    pub fn set_latch(&mut self, on: bool) -> Option<(u8, u8)> {
        self.latch_armed = on;
        if !on && self.held_len == 0 {
            self.stop()
        } else {
            None
        }
    }

    pub fn set_order(&mut self, order: ArpOrder) {
        self.order = order;
        self.rebuild_pool();
        self.reset_walk();
    }

    pub fn set_division(&mut self, division: ArpDivision) {
        self.division = division;
    }

    pub fn set_octaves(&mut self, octaves: u8) {
        self.octaves = octaves.min(MAX_ARP_OCTAVES);
        self.rebuild_pool();
        if self.pool_len > 0 {
            self.cursor %= self.pool_len;
        }
    }

    pub fn set_gate(&mut self, gate: u8) {
        self.gate = gate.clamp(1, 127);
    }

    pub fn set_len(&mut self, len: u8) {
        self.step_len = len.min(MAX_ARP_STEPS as u8);
        if self.step_len == 0 {
            self.step_len = 1;
            self.steps[0] = 0;
        }
        self.rebuild_pool();
        if self.pool_len > 0 {
            self.cursor %= self.pool_len;
        }
    }

    pub fn set_step(&mut self, index: u8, interval: i8) {
        let i = index as usize;
        if i >= MAX_ARP_STEPS {
            return;
        }
        self.steps[i] = interval.clamp(-24, 24);
        if (index + 1) > self.step_len {
            self.step_len = index + 1;
        }
        self.rebuild_pool();
    }

    pub fn note_on(&mut self, note: u8, velocity: u8, channel: u8, at_frame: u64) -> bool {
        if !self.enabled {
            return false;
        }
        let note = note & 0x7f;
        let velocity = velocity.max(1).min(127);
        self.channel = channel & 0x0f;
        self.remember_held(note, velocity);
        let first = !self.running;
        self.set_root(note, velocity);
        self.latched = self.latch_armed;
        if first {
            self.start(at_frame);
            return true;
        }
        false
    }

    /// Sound the first step now and schedule the rest on the musical grid.
    pub fn take_attack(&mut self, at_frame: u64, step_frames: u64) -> Option<(u8, u8, u8)> {
        if !self.running || self.pool_len == 0 {
            return None;
        }
        let note = self.advance_and_peek();
        self.sounding = Some(note);
        let step = step_frames.max(1);
        let gate_frames = ((step as u128 * self.gate as u128) / 127).max(1) as u64;
        let grid = step as f64;
        self.next_on_frame = (((at_frame as f64 / grid).floor() + 1.0) * grid).round() as u64;
        self.next_on_frame = self.next_on_frame.max(at_frame.saturating_add(1));
        if self.gate < 127 {
            self.next_off_frame = Some(at_frame.saturating_add(gate_frames.min(step.saturating_sub(1))));
        } else {
            self.next_off_frame = None;
        }
        Some((self.channel, note, self.velocity))
    }

    pub fn note_off(&mut self, note: u8) -> Option<(u8, u8)> {
        if !self.enabled {
            return None;
        }
        self.forget_held(note & 0x7f);
        if self.held_len == 0 && !self.latch_armed {
            self.stop()
        } else {
            None
        }
    }

    /// Stop the clock. Returns the note still held so the engine can release it.
    pub fn stop(&mut self) -> Option<(u8, u8)> {
        let sounding = self.sounding.take().map(|note| (self.channel, note));
        self.running = false;
        self.latched = false;
        self.next_off_frame = None;
        sounding
    }

    pub fn panic(&mut self) -> Option<(u8, u8)> {
        self.held_len = 0;
        self.stop()
    }

    /// Push note-on/off events that fall inside this audio block.
    pub fn collect(
        &mut self,
        transport: &Transport,
        block_start: u64,
        frames: u32,
        out: &mut [ArpEvent],
    ) -> usize {
        if !self.running || self.pool_len == 0 || out.is_empty() {
            return 0;
        }
        let block_end = block_start.saturating_add(frames as u64);
        let step = transport
            .ticks_to_samples(self.division.ticks())
            .round()
            .max(1.0) as u64;
        let gate_frames = ((step as u128 * self.gate as u128) / 127).max(1) as u64;
        let mut len = 0;

        if let Some(off_at) = self.next_off_frame {
            if off_at < block_end && len < out.len() {
                if let Some(note) = self.sounding.take() {
                    let frame = if off_at >= block_start {
                        (off_at - block_start) as u32
                    } else {
                        0
                    };
                    out[len] = ArpEvent {
                        frame,
                        channel: self.channel,
                        note,
                        velocity: 0,
                        on: false,
                    };
                    len += 1;
                }
                self.next_off_frame = None;
            }
        }

        while self.next_on_frame < block_end && len + 1 < out.len() {
            let frame = if self.next_on_frame >= block_start {
                (self.next_on_frame - block_start) as u32
            } else {
                0
            };
            if let Some(note) = self.sounding.take() {
                out[len] = ArpEvent {
                    frame,
                    channel: self.channel,
                    note,
                    velocity: 0,
                    on: false,
                };
                len += 1;
            }
            let note = self.advance_and_peek();
            out[len] = ArpEvent {
                frame,
                channel: self.channel,
                note,
                velocity: self.velocity,
                on: true,
            };
            len += 1;
            self.sounding = Some(note);
            let off_at = self
                .next_on_frame
                .saturating_add(gate_frames.min(step.saturating_sub(1)));
            if self.gate < 127 && off_at < block_end && len < out.len() {
                out[len] = ArpEvent {
                    frame: if off_at >= block_start {
                        (off_at - block_start) as u32
                    } else {
                        0
                    },
                    channel: self.channel,
                    note,
                    velocity: 0,
                    on: false,
                };
                len += 1;
                self.sounding = None;
                self.next_off_frame = None;
            } else if self.gate < 127 {
                self.next_off_frame = Some(off_at);
            } else {
                self.next_off_frame = None;
            }
            self.next_on_frame = self.next_on_frame.saturating_add(step);
        }
        len
    }

    fn start(&mut self, at_frame: u64) {
        self.rebuild_pool();
        self.reset_walk();
        self.running = self.pool_len > 0;
        self.next_on_frame = at_frame;
        self.next_off_frame = None;
        self.sounding = None;
    }

    fn set_root(&mut self, note: u8, velocity: u8) {
        self.root = note;
        self.velocity = velocity;
        self.rebuild_pool();
        if self.pool_len > 0 {
            self.cursor %= self.pool_len;
        }
    }

    fn remember_held(&mut self, note: u8, velocity: u8) {
        if let Some(slot) = self.held.iter_mut().take(self.held_len as usize).find(|h| h.note == note)
        {
            slot.velocity = velocity;
            return;
        }
        if (self.held_len as usize) < MAX_ARP_HELD {
            self.held[self.held_len as usize] = HeldKey { note, velocity };
            self.held_len += 1;
        } else {
            self.held[MAX_ARP_HELD - 1] = HeldKey { note, velocity };
        }
    }

    fn forget_held(&mut self, note: u8) {
        let mut w = 0u8;
        for r in 0..self.held_len {
            if self.held[r as usize].note != note {
                self.held[w as usize] = self.held[r as usize];
                w += 1;
            }
        }
        self.held_len = w;
    }

    fn reset_walk(&mut self) {
        if self.pool_len == 0 {
            self.cursor = 0;
            self.dir = 1;
            return;
        }
        if self.order.starts_at_top() {
            self.cursor = self.pool_len - 1;
            self.dir = -1;
        } else {
            self.cursor = 0;
            self.dir = 1;
        }
    }

    fn rebuild_pool(&mut self) {
        self.pool_len = 0;
        let octaves = self.octaves.min(MAX_ARP_OCTAVES);
        let mut raw = [0u8; MAX_ARP_POOL];
        let mut raw_len = 0usize;
        for oct in 0..=octaves {
            for i in 0..self.step_len as usize {
                let shifted = self.root as i16 + self.steps[i] as i16 + (oct as i16) * 12;
                if (0..=127).contains(&shifted) && raw_len < MAX_ARP_POOL {
                    raw[raw_len] = shifted as u8;
                    raw_len += 1;
                }
            }
        }
        if raw_len == 0 {
            return;
        }
        if self.order == ArpOrder::Order {
            self.pool[..raw_len].copy_from_slice(&raw[..raw_len]);
            self.pool_len = raw_len as u8;
            return;
        }
        raw[..raw_len].sort_unstable();
        let mut unique = 0usize;
        for i in 0..raw_len {
            if unique == 0 || raw[i] != self.pool[unique - 1] {
                self.pool[unique] = raw[i];
                unique += 1;
            }
        }
        self.pool_len = unique as u8;
    }

    fn advance_and_peek(&mut self) -> u8 {
        if self.pool_len == 0 {
            return self.root;
        }
        if self.order == ArpOrder::Random {
            return self.random_note();
        }
        let note = self.pool[self.cursor as usize];
        self.step_cursor();
        note
    }

    fn random_note(&mut self) -> u8 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let n = self.pool_len as u32;
        let pick = (self.rng >> 16) % n.max(1);
        if n > 1 && pick == self.cursor as u32 {
            self.cursor = ((pick + 1) % n) as u8;
        } else {
            self.cursor = pick as u8;
        }
        self.pool[self.cursor as usize]
    }

    fn step_cursor(&mut self) {
        let n = self.pool_len as i16;
        if n <= 1 {
            self.cursor = 0;
            return;
        }
        if !self.order.bounces() {
            if self.dir >= 0 {
                self.cursor = (self.cursor + 1) % self.pool_len;
            } else {
                self.cursor = if self.cursor == 0 {
                    self.pool_len - 1
                } else {
                    self.cursor - 1
                };
            }
            return;
        }
        let next = self.cursor as i16 + self.dir as i16;
        if next >= n || next < 0 {
            if self.order.inclusive() {
                self.dir = -self.dir;
            } else {
                self.dir = -self.dir;
                let bounced = self.cursor as i16 + self.dir as i16;
                self.cursor = bounced.clamp(0, n - 1) as u8;
            }
        } else {
            self.cursor = next as u8;
        }
    }
}

/// Walk helper used by tests — expand + walk `count` notes without the clock.
pub fn walk_notes(order: ArpOrder, root: u8, steps: &[i8], octaves: u8, count: usize) -> Vec<u8> {
    let mut arp = Arpeggiator::new();
    arp.set_enabled(true);
    arp.set_octaves(octaves);
    arp.set_len(steps.len() as u8);
    for (i, interval) in steps.iter().enumerate() {
        arp.set_step(i as u8, *interval);
    }
    arp.set_order(order);
    arp.set_root(root, 110);
    arp.reset_walk();
    (0..count).map(|_| arp.advance_and_peek()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport() -> Transport {
        let mut t = Transport::new(48_000.0);
        t.set_bpm(120.0);
        t
    }

    #[test]
    fn major_triad_up_then_wraps() {
        assert_eq!(
            walk_notes(ArpOrder::Up, 60, &[0, 4, 7], 0, 6),
            vec![60, 64, 67, 60, 64, 67]
        );
    }

    #[test]
    fn down_starts_at_the_top() {
        assert_eq!(
            walk_notes(ArpOrder::Down, 60, &[0, 4, 7], 0, 4),
            vec![67, 64, 60, 67]
        );
    }

    #[test]
    fn inclusive_up_repeats_the_ends() {
        assert_eq!(
            walk_notes(ArpOrder::InclUp, 60, &[0, 4, 7], 0, 8),
            vec![60, 64, 67, 67, 64, 60, 60, 64]
        );
    }

    #[test]
    fn exclusive_up_skips_the_ends() {
        assert_eq!(
            walk_notes(ArpOrder::ExclUp, 60, &[0, 4, 7], 0, 6),
            vec![60, 64, 67, 64, 60, 64]
        );
    }

    #[test]
    fn inclusive_down_starts_high_and_repeats_ends() {
        assert_eq!(
            walk_notes(ArpOrder::InclDown, 60, &[0, 4, 7], 0, 6),
            vec![67, 64, 60, 60, 64, 67]
        );
    }

    #[test]
    fn exclusive_down_starts_high_and_skips_ends() {
        assert_eq!(
            walk_notes(ArpOrder::ExclDown, 60, &[0, 4, 7], 0, 5),
            vec![67, 64, 60, 64, 67]
        );
    }

    #[test]
    fn order_keeps_written_intervals_before_sorting() {
        assert_eq!(
            walk_notes(ArpOrder::Order, 60, &[7, 0, 4], 0, 3),
            vec![67, 60, 64]
        );
        assert_eq!(
            walk_notes(ArpOrder::Up, 60, &[7, 0, 4], 0, 3),
            vec![60, 64, 67]
        );
    }

    #[test]
    fn extra_octave_appends_the_shape() {
        assert_eq!(
            walk_notes(ArpOrder::Order, 60, &[0, 4], 1, 4),
            vec![60, 64, 72, 76]
        );
    }

    #[test]
    fn latched_root_retarget_keeps_the_shape() {
        let mut arp = Arpeggiator::new();
        arp.set_enabled(true);
        arp.set_latch(true);
        arp.set_order(ArpOrder::Order);
        arp.set_octaves(0);
        arp.note_on(60, 100, 0, 0);
        assert_eq!(arp.advance_and_peek(), 60);
        assert_eq!(arp.advance_and_peek(), 64);
        arp.note_off(60);
        assert!(arp.running());
        assert!(arp.latched());
        arp.note_on(62, 110, 0, 100);
        assert_eq!(arp.root(), 62);
        arp.reset_walk();
        assert_eq!(arp.advance_and_peek(), 62);
        assert_eq!(arp.advance_and_peek(), 66);
        assert_eq!(arp.advance_and_peek(), 69);
    }

    #[test]
    fn unlatched_release_stops() {
        let mut arp = Arpeggiator::new();
        arp.set_enabled(true);
        arp.set_latch(false);
        arp.note_on(60, 100, 0, 0);
        assert!(arp.running());
        arp.note_off(60);
        assert!(!arp.running());
    }

    #[test]
    fn incoming_note_off_does_not_clear_a_sounding_step_of_the_same_pitch() {
        let mut arp = Arpeggiator::new();
        let transport = transport();
        arp.set_enabled(true);
        arp.set_latch(true);
        arp.set_order(ArpOrder::Order);
        arp.set_octaves(0);
        arp.set_gate(127);
        arp.note_on(60, 100, 0, 0);
        let mut events = [ArpEvent {
            frame: 0,
            channel: 0,
            note: 0,
            velocity: 0,
            on: false,
        }; MAX_ARP_EVENTS_PER_BLOCK];
        let n = arp.collect(&transport, 0, 64, &mut events);
        assert!(n >= 1);
        assert!(events[0].on);
        assert_eq!(events[0].note, 60);
        arp.note_off(60);
        assert_eq!(arp.sounding, Some(60));
        assert!(arp.running());
    }

    #[test]
    fn overdue_gate_off_is_flushed_at_the_next_block() {
        let transport = transport();
        let mut arp = Arpeggiator::new();
        arp.set_enabled(true);
        arp.set_gate(40);
        arp.set_division(ArpDivision::Sixteenth);
        assert!(arp.note_on(60, 100, 0, 100));
        let step = transport
            .ticks_to_samples(ArpDivision::Sixteenth.ticks())
            .round() as u64;
        let first = arp.take_attack(100, step).unwrap();
        assert_eq!(first.1, 60);
        let mut events = [ArpEvent {
            frame: 0,
            channel: 0,
            note: 0,
            velocity: 0,
            on: false,
        }; MAX_ARP_EVENTS_PER_BLOCK];
        let n = arp.collect(&transport, 10_000, 256, &mut events);
        assert!(
            events.iter().take(n).any(|e| !e.on && e.note == 60),
            "a gate-off that landed in the previous block must still be emitted, got {:?}",
            &events[..n]
        );
    }

    #[test]
    fn sixteenth_hits_land_on_the_grid() {
        let transport = transport();
        let mut arp = Arpeggiator::new();
        arp.set_enabled(true);
        arp.set_division(ArpDivision::Sixteenth);
        arp.set_gate(64);
        arp.note_on(60, 100, 0, 0);
        let mut events = [ArpEvent {
            frame: 0,
            channel: 0,
            note: 0,
            velocity: 0,
            on: false,
        }; MAX_ARP_EVENTS_PER_BLOCK];
        let n = arp.collect(&transport, 0, 18_000, &mut events);
        let ons: Vec<_> = events.iter().take(n).filter(|e| e.on).collect();
        assert!(ons.len() >= 3, "got {} ons", ons.len());
        let step = transport
            .ticks_to_samples(ArpDivision::Sixteenth.ticks())
            .round() as u32;
        assert_eq!(ons[1].frame - ons[0].frame, step);
    }
}
