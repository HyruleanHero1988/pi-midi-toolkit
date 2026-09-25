//! EP-style punch-in FX on the master bus.
//!
//! Eight pads, layered. Buffer effects (repeat / tape / drop) share one ring
//! and take priority in that order. Filters, crush, and slice run after.
//! SEND is a wet-boost of the existing bus insert (handled by the engine).
//!
//! Amounts are 0..1. The UI maps pad Y (and latch) onto them; this unit only
//! hears the current amount per slot. `process` never allocates.

pub const PUNCH_PAD_COUNT: usize = 8;
pub const PUNCH_RPT: u8 = 0;
pub const PUNCH_TAPE: u8 = 1;
pub const PUNCH_LPF: u8 = 2;
pub const PUNCH_HPF: u8 = 3;
pub const PUNCH_SEND: u8 = 4;
pub const PUNCH_SLICE: u8 = 5;
pub const PUNCH_DROP: u8 = 6;
pub const PUNCH_CRUSH: u8 = 7;

const RING_SEC: f32 = 2.0;
const ENGAGE: f32 = 0.04;

pub const PUNCH_LABELS: [&str; PUNCH_PAD_COUNT] =
    ["RPT", "TAPE", "LPF", "HPF", "SEND", "SLICE", "DROP", "CRUSH"];
/// MPK mini factory knobs 1–8 (Prog Select → Pad 1).
pub const PUNCH_KNOB_CCS: [u8; PUNCH_PAD_COUNT] = [70, 71, 72, 73, 74, 75, 76, 77];
/// MPK factory Bank A pads, row-swapped to match the FX grid.
pub const PUNCH_PAD_NOTES: [u8; PUNCH_PAD_COUNT] = [40, 41, 42, 43, 36, 37, 38, 39];

pub fn punch_index_for_knob_cc(controller: u8) -> Option<usize> {
    PUNCH_KNOB_CCS.iter().position(|cc| *cc == controller)
}

pub fn punch_index_for_pad_note(note: u8) -> Option<usize> {
    PUNCH_PAD_NOTES.iter().position(|n| *n == note)
}

/// Master-bus punch-in processor. Sized in [`PunchRack::new`].
pub struct PunchRack {
    sample_rate: f32,
    amount: [f32; PUNCH_PAD_COUNT],
    ring: Vec<f32>,
    write: usize,
    rpt_on: bool,
    rpt_start: usize,
    rpt_len: usize,
    rpt_read: f32,
    tape_on: bool,
    tape_rate: f32,
    tape_read: f32,
    drop_read: f32,
    lp_l: f32,
    lp_b: f32,
    hp_l: f32,
    hp_b: f32,
    slice_phase: f32,
    crush_hold: f32,
    crush_left: u32,
}

impl PunchRack {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(8000.0);
        let n = (sample_rate * RING_SEC) as usize + 64;
        Self {
            sample_rate,
            amount: [0.0; PUNCH_PAD_COUNT],
            ring: vec![0.0; n.max(64)],
            write: 0,
            rpt_on: false,
            rpt_start: 0,
            rpt_len: 1,
            rpt_read: 0.0,
            tape_on: false,
            tape_rate: 1.0,
            tape_read: 0.0,
            drop_read: 0.0,
            lp_l: 0.0,
            lp_b: 0.0,
            hp_l: 0.0,
            hp_b: 0.0,
            slice_phase: 0.0,
            crush_hold: 0.0,
            crush_left: 0,
        }
    }

    pub fn amount(&self, slot: u8) -> f32 {
        self.amount[slot_index(slot)]
    }

    pub fn send_amount(&self) -> f32 {
        self.amount[PUNCH_SEND as usize]
    }

    pub fn set_amount(&mut self, slot: u8, value: f32) {
        self.amount[slot_index(slot)] = value.clamp(0.0, 1.0);
    }

    pub fn clear(&mut self) {
        self.amount = [0.0; PUNCH_PAD_COUNT];
        self.rpt_on = false;
        self.tape_on = false;
        self.tape_rate = 1.0;
        self.slice_phase = 0.0;
        self.crush_left = 0;
    }

    /// Drop the ring so a panic cannot leak a frozen loop.
    pub fn reset(&mut self) {
        self.clear();
        self.ring.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
        self.rpt_read = 0.0;
        self.tape_read = 0.0;
        self.drop_read = 0.0;
        self.lp_l = 0.0;
        self.lp_b = 0.0;
        self.hp_l = 0.0;
        self.hp_b = 0.0;
        self.crush_hold = 0.0;
    }

    pub fn is_idle(&self) -> bool {
        self.amount.iter().all(|a| *a <= ENGAGE) && !self.tape_on && !self.rpt_on
    }

    /// Process in place. `bpm` drives repeat / slice grid. Allocation-free.
    pub fn process(&mut self, buf: &mut [f32], bpm: f32) {
        if buf.is_empty() {
            return;
        }
        let sr = self.sample_rate;
        let nring = self.ring.len();
        let rpt = self.amount[PUNCH_RPT as usize];
        let tape = self.amount[PUNCH_TAPE as usize];
        let drop = self.amount[PUNCH_DROP as usize];

        if tape > ENGAGE && !self.tape_on {
            self.tape_on = true;
            self.tape_rate = 1.0;
            self.tape_read = self.write as f32;
        } else if tape <= ENGAGE {
            self.tape_on = false;
            self.tape_rate = 1.0;
        }

        let want_rpt = rpt > ENGAGE && !self.tape_on;
        if want_rpt && !self.rpt_on {
            self.rpt_len = rpt_loop_samples(rpt, bpm, sr).clamp(32, nring - 1);
            self.rpt_start = (self.write + nring - self.rpt_len) % nring;
            self.rpt_read = 0.0;
            self.rpt_on = true;
        } else if want_rpt {
            let len = rpt_loop_samples(rpt, bpm, sr).clamp(32, nring - 1);
            if len != self.rpt_len {
                self.rpt_len = len;
                self.rpt_read %= self.rpt_len as f32;
            }
        } else {
            self.rpt_on = false;
        }

        if drop > ENGAGE && !self.tape_on && !self.rpt_on {
            // Keep the read head a short grain behind the write head.
            if self.drop_read == 0.0 {
                self.drop_read = self.write as f32;
            }
        }

        let tape_target = tape_target_rate(tape);
        // Motor slew: heavier Y drags more, but the deck never parks.
        let tape_slew = 1.0 - (-1.0 / (0.14 * sr).max(1.0)).exp();
        let drop_rate = if drop > ENGAGE {
            2f32.powf(-drop * 1.7)
        } else {
            1.0
        };

        for s in buf.iter_mut() {
            self.ring[self.write] = *s;
            let live = *s;
            self.write = (self.write + 1) % nring;

            let wet = if self.tape_on {
                self.tape_rate += (tape_target - self.tape_rate) * tape_slew;
                let sample = read_ring(&self.ring, self.tape_read);
                self.tape_read = (self.tape_read + self.tape_rate).rem_euclid(nring as f32);
                sample
            } else if self.rpt_on {
                let sample = read_ring(
                    &self.ring,
                    self.rpt_start as f32 + self.rpt_read,
                );
                self.rpt_read += 1.0;
                if self.rpt_read >= self.rpt_len as f32 {
                    self.rpt_read -= self.rpt_len as f32;
                }
                sample
            } else if drop > ENGAGE {
                let sample = read_ring(&self.ring, self.drop_read);
                self.drop_read = (self.drop_read + drop_rate).rem_euclid(nring as f32);
                sample
            } else {
                self.drop_read = 0.0;
                live
            };
            *s = wet;
        }

        let lpf = self.amount[PUNCH_LPF as usize];
        if lpf > ENGAGE {
            apply_svf_lowpass(buf, 1.0 - lpf * 0.92, &mut self.lp_l, &mut self.lp_b, sr);
        }
        let hpf = self.amount[PUNCH_HPF as usize];
        if hpf > ENGAGE {
            apply_svf_highpass(buf, hpf, &mut self.hp_l, &mut self.hp_b, sr);
        }
        let crush = self.amount[PUNCH_CRUSH as usize];
        if crush > ENGAGE {
            apply_crush(buf, crush, &mut self.crush_hold, &mut self.crush_left);
        }
        let slice = self.amount[PUNCH_SLICE as usize];
        if slice > ENGAGE {
            apply_slice(buf, slice, bpm, sr, &mut self.slice_phase);
        }
    }
}

/// Light press is a slight drag; full press is slow-mo. Never reaches 0.
fn tape_target_rate(amount: f32) -> f32 {
    let t = amount.clamp(0.0, 1.0);
    0.88 - t * 0.60
}

fn slot_index(slot: u8) -> usize {
    (slot as usize).min(PUNCH_PAD_COUNT - 1)
}

fn read_ring(ring: &[f32], pos: f32) -> f32 {
    let n = ring.len() as f32;
    let p = pos.rem_euclid(n);
    let i0 = p.floor() as usize;
    let i1 = (i0 + 1) % ring.len();
    let frac = p - i0 as f32;
    ring[i0] * (1.0 - frac) + ring[i1] * frac
}

/// 1/4 → 1/8 → 1/16 → 1/32 of a beat as amount rises.
fn rpt_loop_samples(amount: f32, bpm: f32, sr: f32) -> usize {
    let steps = [1.0f32, 0.5, 0.25, 0.125];
    let idx = (amount.clamp(0.0, 1.0) * 3.999).floor() as usize;
    let beats = steps[idx.min(3)];
    let sec = beats * 60.0 / bpm.max(20.0);
    (sec * sr).round() as usize
}

fn apply_svf_lowpass(buf: &mut [f32], tone: f32, lp: &mut f32, bp: &mut f32, sample_rate: f32) {
    let tone = tone.clamp(0.0, 1.0);
    if tone >= 0.985 {
        *lp = buf.last().copied().unwrap_or(*lp);
        *bp = 0.0;
        return;
    }
    let sr = sample_rate.max(8000.0);
    let fc = 90.0 * (8000.0_f32 / 90.0).powf(tone);
    let fc = fc.min(sr * 0.14);
    let f = (2.0 * std::f32::consts::PI * fc / sr).sin();
    let damp = 0.38 + 0.62 * tone;
    let mut l = *lp;
    let mut b = *bp;
    for s in buf.iter_mut() {
        l += f * b;
        let hp = *s - l - damp * b;
        b += f * hp;
        *s = l;
    }
    *lp = l;
    *bp = b;
}

fn apply_svf_highpass(buf: &mut [f32], amount: f32, lp: &mut f32, bp: &mut f32, sample_rate: f32) {
    let amount = amount.clamp(0.0, 1.0);
    let sr = sample_rate.max(8000.0);
    let fc = 40.0 * (4000.0_f32 / 40.0).powf(amount);
    let fc = fc.min(sr * 0.20);
    let f = (2.0 * std::f32::consts::PI * fc / sr).sin();
    let damp = 0.45;
    let mut l = *lp;
    let mut b = *bp;
    for s in buf.iter_mut() {
        l += f * b;
        let hp = *s - l - damp * b;
        b += f * hp;
        *s = hp;
    }
    *lp = l;
    *bp = b;
}

fn apply_crush(buf: &mut [f32], amount: f32, hold: &mut f32, left: &mut u32) {
    let bits = 12.0 - amount * 10.0;
    let levels = 2f32.powf(bits.clamp(1.5, 12.0));
    let hold_n = 1 + (amount * 28.0) as u32;
    for s in buf.iter_mut() {
        if *left == 0 {
            *hold = (*s * levels).round() / levels;
            *left = hold_n;
        }
        *s = *hold;
        *left = left.saturating_sub(1);
    }
}

fn apply_slice(buf: &mut [f32], amount: f32, bpm: f32, sr: f32, phase: &mut f32) {
    let steps = [1.0f32, 0.5, 0.25, 0.125];
    let idx = (amount.clamp(0.0, 1.0) * 3.999).floor() as usize;
    let beats = steps[idx.min(3)];
    let period = (beats * 60.0 / bpm.max(20.0) * sr).max(8.0);
    let inc = 1.0 / period;
    for s in buf.iter_mut() {
        *phase += inc;
        if *phase >= 1.0 {
            *phase -= 1.0;
        }
        // Hard gate, ~45% open — EP pad 7 style amplitude chop.
        if *phase > 0.45 {
            *s = 0.0;
        }
    }
}

/// Linear interpolation of bus wet toward full when SEND is down.
pub fn boost_send_mix(base: f32, send: f32) -> f32 {
    let send = send.clamp(0.0, 1.0);
    (base + send * (1.0 - base) * 0.95).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpk_factory_knobs_and_bank_a_map_left_to_right() {
        assert_eq!(punch_index_for_knob_cc(70), Some(PUNCH_RPT as usize));
        assert_eq!(punch_index_for_knob_cc(77), Some(PUNCH_CRUSH as usize));
        assert_eq!(punch_index_for_pad_note(40), Some(PUNCH_RPT as usize));
        assert_eq!(punch_index_for_pad_note(36), Some(PUNCH_SEND as usize));
        assert_eq!(punch_index_for_pad_note(39), Some(PUNCH_CRUSH as usize));
        assert!(punch_index_for_knob_cc(1).is_none());
        assert!(punch_index_for_pad_note(60).is_none());
    }

    #[test]
    fn idle_rack_leaves_signal() {
        let mut p = PunchRack::new(48_000.0);
        let mut buf = [0.4f32; 64];
        p.process(&mut buf, 120.0);
        assert!(buf.iter().all(|v| (*v - 0.4).abs() < 1e-6));
    }

    #[test]
    fn beat_repeat_loops_a_captured_impulse() {
        let mut p = PunchRack::new(48_000.0);
        let loop_n = rpt_loop_samples(0.1, 120.0, 48_000.0);
        // Prime the ring with silence, then one hit, then engage.
        let mut prime = vec![0.0f32; loop_n];
        prime[0] = 1.0;
        p.process(&mut prime, 120.0);
        p.set_amount(PUNCH_RPT, 0.1);
        let mut out = vec![0.0f32; loop_n * 2];
        p.process(&mut out, 120.0);
        let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.5, "repeat should reprint the hit, peak={peak}");
        // Second loop pass should also carry energy.
        let late = out[loop_n..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(late > 0.5, "loop must wrap, late={late}");
    }

    #[test]
    fn tape_drag_stays_audible() {
        let mut p = PunchRack::new(48_000.0);
        let mut prime = vec![0.3f32; 2048];
        p.process(&mut prime, 120.0);
        p.set_amount(PUNCH_TAPE, 1.0);
        let mut out = vec![0.3f32; 48_000];
        p.process(&mut out, 120.0);
        let tail = out[out.len() - 64..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(tail > 0.15, "full tape must keep moving, tail={tail}");
        assert!(tape_target_rate(1.0) > 0.2);
        assert!(tape_target_rate(0.1) > tape_target_rate(1.0));
    }

    #[test]
    fn lpf_darkens_a_hot_signal() {
        let mut p = PunchRack::new(48_000.0);
        p.set_amount(PUNCH_LPF, 1.0);
        let mut buf = vec![0.0f32; 256];
        for (i, s) in buf.iter_mut().enumerate() {
            *s = if i % 2 == 0 { 0.8 } else { -0.8 };
        }
        p.process(&mut buf, 120.0);
        let peak = buf.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak < 0.55, "deep LPF should kill the Nyquist square, peak={peak}");
    }

    #[test]
    fn crush_quantizes_amplitude() {
        let mut p = PunchRack::new(48_000.0);
        p.set_amount(PUNCH_CRUSH, 1.0);
        let mut buf = [0.37f32; 32];
        p.process(&mut buf, 120.0);
        let unique: Vec<i32> = {
            let mut v: Vec<i32> = buf.iter().map(|s| (s * 1000.0).round() as i32).collect();
            v.sort();
            v.dedup();
            v
        };
        assert!(
            unique.len() <= 4,
            "heavy crush should collapse levels, got {unique:?}"
        );
    }

    #[test]
    fn slice_gates_a_steady_tone() {
        let mut p = PunchRack::new(48_000.0);
        p.set_amount(PUNCH_SLICE, 0.9);
        let mut buf = vec![0.5f32; 8000];
        p.process(&mut buf, 120.0);
        let silent = buf.iter().filter(|s| s.abs() < 1e-5).count();
        let loud = buf.iter().filter(|s| s.abs() > 0.4).count();
        assert!(silent > 1000, "slicer must close, silent={silent}");
        assert!(loud > 1000, "slicer must open, loud={loud}");
    }

    #[test]
    fn send_boost_opens_a_dry_mix() {
        assert!((boost_send_mix(0.0, 1.0) - 0.95).abs() < 1e-5);
        assert!(boost_send_mix(0.4, 0.0) < 0.41);
        assert!(boost_send_mix(0.4, 1.0) > 0.9);
    }

    #[test]
    fn reset_kills_a_latched_repeat() {
        let mut p = PunchRack::new(48_000.0);
        p.set_amount(PUNCH_RPT, 0.8);
        let mut buf = [0.6f32; 128];
        p.process(&mut buf, 120.0);
        p.reset();
        assert!(p.is_idle());
        let mut quiet = [0.2f32; 64];
        p.process(&mut quiet, 120.0);
        assert!(quiet.iter().all(|v| (*v - 0.2).abs() < 1e-6));
    }
}
