//! Lock-free latest-value slots for continuous XY.
//!
//! Reliable edges travel on the command ring. Finger motion overwrites a slot
//! so the audio thread applies only the newest sample per gesture.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use jambox_core::LatestTouch;

pub const MAX_MAILBOX_SLOTS: usize = 8;

struct Slot {
    /// 0 = empty, otherwise `owner + 1`.
    owner: AtomicU32,
    xy: AtomicU64,
    meta: AtomicU32,
}

impl Slot {
    fn empty() -> Self {
        Self {
            owner: AtomicU32::new(0),
            xy: AtomicU64::new(0),
            meta: AtomicU32::new(0),
        }
    }
}

pub struct LatestMailbox {
    slots: [Slot; MAX_MAILBOX_SLOTS],
    overwrites: AtomicU64,
}

impl Default for LatestMailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl LatestMailbox {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| Slot::empty()),
            overwrites: AtomicU64::new(0),
        }
    }

    pub fn overwrites(&self) -> u64 {
        self.overwrites.load(Ordering::Relaxed)
    }

    pub fn publish(&self, touch: LatestTouch) {
        let touch = touch.clamp();
        let key = touch.owner.saturating_add(1);
        let xy = ((touch.x.to_bits() as u64) << 32) | touch.y.to_bits() as u64;
        let meta = touch.channel as u32 | (touch.velocity as u32) << 8;

        for slot in &self.slots {
            if slot.owner.load(Ordering::Acquire) == key {
                slot.xy.store(xy, Ordering::Release);
                slot.meta.store(meta, Ordering::Release);
                self.overwrites.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        for slot in &self.slots {
            if slot
                .owner
                .compare_exchange(0, key, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                slot.xy.store(xy, Ordering::Release);
                slot.meta.store(meta, Ordering::Release);
                return;
            }
        }
        self.slots[0].xy.store(xy, Ordering::Release);
        self.slots[0].meta.store(meta, Ordering::Release);
        self.slots[0].owner.store(key, Ordering::Release);
        self.overwrites.fetch_add(1, Ordering::Relaxed);
    }

    pub fn clear(&self, owner: u32) {
        let key = owner.saturating_add(1);
        for slot in &self.slots {
            if slot.owner.load(Ordering::Acquire) == key {
                slot.owner.store(0, Ordering::Release);
            }
        }
    }

    pub fn clear_all(&self) {
        for slot in &self.slots {
            slot.owner.store(0, Ordering::Release);
        }
    }

    pub fn snapshot(&self, out: &mut [LatestTouch]) -> usize {
        let mut n = 0;
        for slot in &self.slots {
            if n >= out.len() {
                break;
            }
            let key = slot.owner.load(Ordering::Acquire);
            if key == 0 {
                continue;
            }
            let xy = slot.xy.load(Ordering::Acquire);
            let meta = slot.meta.load(Ordering::Acquire);
            out[n] = LatestTouch {
                owner: key - 1,
                x: f32::from_bits((xy >> 32) as u32),
                y: f32::from_bits(xy as u32),
                channel: meta as u8,
                velocity: (meta >> 8) as u8,
            };
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(owner: u32, x: f32) -> LatestTouch {
        LatestTouch {
            owner,
            x,
            y: 0.5,
            channel: 0,
            velocity: 100,
        }
    }

    #[test]
    fn a_second_write_replaces_the_first() {
        let box_ = LatestMailbox::new();
        box_.publish(touch(4, 0.1));
        box_.publish(touch(4, 0.9));
        let mut out = [touch(0, 0.0); 8];
        assert_eq!(box_.snapshot(&mut out), 1);
        assert!((out[0].x - 0.9).abs() < 1e-5);
        assert!(box_.overwrites() >= 1);
    }

    #[test]
    fn two_gestures_keep_independent_slots() {
        let box_ = LatestMailbox::new();
        box_.publish(touch(1, 0.2));
        box_.publish(touch(2, 0.8));
        let mut out = [touch(0, 0.0); 8];
        assert_eq!(box_.snapshot(&mut out), 2);
        box_.clear(1);
        assert_eq!(box_.snapshot(&mut out), 1);
        assert_eq!(out[0].owner, 2);
    }
}
