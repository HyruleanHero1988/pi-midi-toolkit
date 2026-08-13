//! Sample-accurate musical clock.
//!
//! The audio callback is the only clock that matters. Wall-clock time and UI
//! threads never advance musical position, which is what keeps a loop from
//! drifting or dropping a beat when the UI is busy.

/// Ticks per quarter note. Clip events are stored in ticks so tempo changes scale.
pub const PPQ: u32 = 960;

/// Launch/stop grid for clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantize {
    /// Act on the next audio frame.
    Off,
    Beat,
    Bar,
}

impl Quantize {
    /// Grid length in ticks. `Off` is zero (no waiting).
    pub fn ticks(self, beats_per_bar: u32) -> u64 {
        match self {
            Self::Off => 0,
            Self::Beat => PPQ as u64,
            Self::Bar => PPQ as u64 * beats_per_bar.max(1) as u64,
        }
    }
}

/// Musical transport driven purely by rendered frames.
#[derive(Debug, Clone)]
pub struct Transport {
    sample_rate: f64,
    bpm: f64,
    beats_per_bar: u32,
    /// Frames rendered since the engine started. Monotonic; never rewound by the UI.
    position: u64,
    running: bool,
}

impl Transport {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate: sample_rate.max(8000.0),
            bpm: 120.0,
            beats_per_bar: 4,
            position: 0,
            running: true,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Re-open at a new device rate. Callers must rescale any cached sample positions.
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate.max(8000.0);
    }

    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    pub fn set_bpm(&mut self, bpm: f64) {
        self.bpm = bpm.clamp(20.0, 400.0);
    }

    pub fn beats_per_bar(&self) -> u32 {
        self.beats_per_bar
    }

    pub fn set_beats_per_bar(&mut self, beats: u32) {
        self.beats_per_bar = beats.clamp(1, 32);
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn running(&self) -> bool {
        self.running
    }

    pub fn set_running(&mut self, running: bool) {
        self.running = running;
    }

    /// Advance after a block has been rendered.
    pub fn advance(&mut self, frames: u64) {
        self.position = self.position.wrapping_add(frames);
    }

    /// Frames per tick at the current tempo.
    pub fn samples_per_tick(&self) -> f64 {
        60.0 / (self.bpm * PPQ as f64) * self.sample_rate
    }

    pub fn samples_per_beat(&self) -> f64 {
        60.0 / self.bpm * self.sample_rate
    }

    pub fn ticks_to_samples(&self, ticks: u64) -> f64 {
        ticks as f64 * self.samples_per_tick()
    }

    /// Absolute frame of the next `q` boundary at or after `from`.
    ///
    /// The grid is anchored at frame 0 so every clip launched with the same
    /// quantize lands on the same grid, regardless of when the UI asked.
    pub fn next_boundary(&self, from: u64, q: Quantize) -> u64 {
        let grid_ticks = q.ticks(self.beats_per_bar);
        if grid_ticks == 0 {
            return from;
        }
        let grid = self.ticks_to_samples(grid_ticks);
        if grid <= 1.0 {
            return from;
        }
        let index = (from as f64 / grid).ceil();
        let boundary = (index * grid).round() as u64;
        if boundary < from {
            from
        } else {
            boundary
        }
    }
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
    fn samples_per_beat_matches_tempo() {
        let t = transport();
        // 120 BPM at 48k → half a second per beat.
        assert!((t.samples_per_beat() - 24_000.0).abs() < 1e-6);
        assert!((t.ticks_to_samples(PPQ as u64) - 24_000.0).abs() < 1e-6);
    }

    #[test]
    fn advance_accumulates_frames() {
        let mut t = transport();
        t.advance(128);
        t.advance(128);
        assert_eq!(t.position(), 256);
    }

    #[test]
    fn beat_boundary_is_anchored_to_zero() {
        let t = transport();
        assert_eq!(t.next_boundary(0, Quantize::Beat), 0);
        assert_eq!(t.next_boundary(1, Quantize::Beat), 24_000);
        assert_eq!(t.next_boundary(24_000, Quantize::Beat), 24_000);
        assert_eq!(t.next_boundary(24_001, Quantize::Beat), 48_000);
    }

    #[test]
    fn bar_boundary_uses_time_signature() {
        let mut t = transport();
        t.set_beats_per_bar(4);
        assert_eq!(t.next_boundary(1, Quantize::Bar), 96_000);
        t.set_beats_per_bar(3);
        assert_eq!(t.next_boundary(1, Quantize::Bar), 72_000);
    }

    #[test]
    fn quantize_off_never_waits() {
        let t = transport();
        assert_eq!(t.next_boundary(12_345, Quantize::Off), 12_345);
    }

    #[test]
    fn tempo_is_clamped_to_sane_range() {
        let mut t = transport();
        t.set_bpm(5.0);
        assert!(t.bpm() >= 20.0);
        t.set_bpm(9_999.0);
        assert!(t.bpm() <= 400.0);
    }
}
