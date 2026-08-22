//! Sample-clocked note-repeat lanes owned by touch contacts.

use crate::transport::{Transport, PPQ};

pub const MAX_REPEAT_LANES: usize = 5;
pub const MAX_REPEAT_EVENTS_PER_BLOCK: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatDivision {
    Quarter,
    Eighth,
    EighthTriplet,
    Sixteenth,
}

impl RepeatDivision {
    pub const fn ticks(self) -> u64 {
        match self {
            Self::Quarter => PPQ as u64,
            Self::Eighth => PPQ as u64 / 2,
            Self::EighthTriplet => PPQ as u64 / 3,
            Self::Sixteenth => PPQ as u64 / 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatEvent {
    pub frame: u32,
    pub owner: u32,
    pub channel: u8,
    pub note: u8,
    pub velocity: u8,
}

#[derive(Debug, Clone, Copy)]
struct RepeatLane {
    active: bool,
    owner: u32,
    channel: u8,
    note: u8,
    velocity: u8,
    division: RepeatDivision,
    next_frame: u64,
}

impl RepeatLane {
    const fn silent() -> Self {
        Self {
            active: false,
            owner: 0,
            channel: 9,
            note: 36,
            velocity: 110,
            division: RepeatDivision::Quarter,
            next_frame: 0,
        }
    }
}

pub struct RepeatRack {
    lanes: [RepeatLane; MAX_REPEAT_LANES],
}

impl Default for RepeatRack {
    fn default() -> Self {
        Self::new()
    }
}

impl RepeatRack {
    pub const fn new() -> Self {
        Self {
            lanes: [RepeatLane::silent(); MAX_REPEAT_LANES],
        }
    }

    pub fn active_count(&self) -> usize {
        self.lanes.iter().filter(|lane| lane.active).count()
    }

    pub fn contains(&self, owner: u32) -> bool {
        self.lanes
            .iter()
            .any(|lane| lane.active && lane.owner == owner)
    }

    /// Arm a lane after an immediate first hit. Subsequent hits land on the
    /// absolute musical grid strictly after `at_frame`.
    pub fn start(
        &mut self,
        owner: u32,
        channel: u8,
        note: u8,
        velocity: u8,
        division: RepeatDivision,
        at_frame: u64,
        transport: &Transport,
    ) {
        self.stop(owner);
        let slot = self
            .lanes
            .iter()
            .position(|lane| !lane.active)
            .unwrap_or(0);
        let grid = transport.ticks_to_samples(division.ticks()).max(1.0);
        let next_frame = (((at_frame as f64 / grid).floor() + 1.0) * grid).round() as u64;
        self.lanes[slot] = RepeatLane {
            active: true,
            owner,
            channel: channel & 0x0f,
            note: note & 0x7f,
            velocity: velocity.clamp(1, 127),
            division,
            next_frame: next_frame.max(at_frame.saturating_add(1)),
        };
    }

    pub fn stop(&mut self, owner: u32) {
        for lane in &mut self.lanes {
            if lane.active && lane.owner == owner {
                *lane = RepeatLane::silent();
            }
        }
    }

    pub fn stop_all(&mut self) {
        self.lanes = [RepeatLane::silent(); MAX_REPEAT_LANES];
    }

    pub fn collect(
        &mut self,
        transport: &Transport,
        block_start: u64,
        frames: u32,
        out: &mut [RepeatEvent],
    ) -> usize {
        let block_end = block_start.saturating_add(frames as u64);
        let mut len = 0;
        for lane in &mut self.lanes {
            if !lane.active {
                continue;
            }
            let step = transport
                .ticks_to_samples(lane.division.ticks())
                .round()
                .max(1.0) as u64;
            while lane.next_frame < block_end && len < out.len() {
                if lane.next_frame >= block_start {
                    out[len] = RepeatEvent {
                        frame: (lane.next_frame - block_start) as u32,
                        owner: lane.owner,
                        channel: lane.channel,
                        note: lane.note,
                        velocity: lane.velocity,
                    };
                    len += 1;
                }
                lane.next_frame = lane.next_frame.saturating_add(step);
            }
        }
        len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport() -> Transport {
        let mut transport = Transport::new(48_000.0);
        transport.set_bpm(120.0);
        transport
    }

    #[test]
    fn quarter_repeat_is_anchored_to_the_next_beat() {
        let transport = transport();
        let mut rack = RepeatRack::new();
        rack.start(
            7,
            9,
            36,
            110,
            RepeatDivision::Quarter,
            1_000,
            &transport,
        );
        let mut events = [RepeatEvent {
            frame: 0,
            owner: 0,
            channel: 0,
            note: 0,
            velocity: 0,
        }; MAX_REPEAT_EVENTS_PER_BLOCK];
        assert_eq!(rack.collect(&transport, 0, 24_000, &mut events), 0);
        assert_eq!(rack.collect(&transport, 24_000, 256, &mut events), 1);
        assert_eq!(events[0].owner, 7);
        assert_eq!(events[0].frame, 0);
    }

    #[test]
    fn stop_cancels_future_hits() {
        let transport = transport();
        let mut rack = RepeatRack::new();
        rack.start(
            3,
            9,
            36,
            100,
            RepeatDivision::Eighth,
            0,
            &transport,
        );
        rack.stop(3);
        let mut events = [RepeatEvent {
            frame: 0,
            owner: 0,
            channel: 0,
            note: 0,
            velocity: 0,
        }; MAX_REPEAT_EVENTS_PER_BLOCK];
        assert_eq!(rack.collect(&transport, 12_000, 512, &mut events), 0);
    }
}
