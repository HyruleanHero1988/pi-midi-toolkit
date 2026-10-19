//! Live vs clip mix sources.
//!
//! MIX mode trims these independently: live keys, live kit, and each clip slot
//! (phrase pads + SEQ) so a running sequence can sit under a live take.

use crate::clip::{MAX_CLIPS, SEQ_CLIP_SLOT, SEQ_DRUM_MIX_SLOT, SEQ_KAOSS_MIX_SLOT};
use crate::DRUM_CHANNEL;

/// Where a sounding voice / drum hit came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MixSource {
    #[default]
    Live,
    Clip(u8),
}

impl MixSource {
    pub fn clip(slot: usize) -> Self {
        Self::Clip((slot.min(MAX_CLIPS.saturating_sub(1))) as u8)
    }

    pub fn gain(self, live: f32, clips: &[f32; MAX_CLIPS]) -> f32 {
        match self {
            Self::Live => live.clamp(0.0, 2.0),
            Self::Clip(slot) => clips[(slot as usize).min(MAX_CLIPS - 1)].clamp(0.0, 2.0),
        }
    }

    /// SEQ / song notes share one clip but three MIX faders.
    pub fn seq_family(slot: usize, channel: u8) -> Self {
        if slot == SEQ_CLIP_SLOT as usize
            || slot == SEQ_DRUM_MIX_SLOT as usize
            || slot == SEQ_KAOSS_MIX_SLOT as usize
        {
            if channel == DRUM_CHANNEL {
                Self::clip(SEQ_DRUM_MIX_SLOT as usize)
            } else {
                Self::clip(SEQ_CLIP_SLOT as usize)
            }
        } else {
            Self::clip(slot)
        }
    }

    pub fn seq_kaoss(slot: usize) -> Self {
        if slot == SEQ_CLIP_SLOT as usize || slot == SEQ_KAOSS_MIX_SLOT as usize {
            Self::clip(SEQ_KAOSS_MIX_SLOT as usize)
        } else {
            Self::clip(slot)
        }
    }
}

/// Left/right sweep gains. `0` = hard left, `0.5` = center (unity both), `1` = hard right.
///
/// Center stays 0 dB on each side so existing dual-mono output level is unchanged.
pub fn sweep_gains(sweep: f32) -> (f32, f32) {
    let t = sweep.clamp(0.0, 1.0);
    if t <= 0.5 {
        (1.0, t * 2.0)
    } else {
        ((1.0 - t) * 2.0, 1.0)
    }
}

/// Keyboard pan 0..1. MIDI 60 (C4) is center; ±2 octaves reach the edges.
pub fn note_sweep(note: u8) -> f32 {
    ((note as f32 - 60.0) * (1.0 / 48.0) + 0.5).clamp(0.0, 1.0)
}

/// How far note position pulls L/R at full width. 1 would hard-pan C2/C6.
const KEYBOARD_PAN: f32 = 0.5;

/// Per-voice gains: C4 stays centered; low notes lean left, high notes right.
pub fn width_gains(note: u8, width: f32) -> (f32, f32) {
    let w = width.clamp(0.0, 1.0);
    let pan = 0.5 + (note_sweep(note) - 0.5) * w * KEYBOARD_PAN;
    sweep_gains(pan)
}

/// Wavetable A vs B stereo. Morph 0 and 1 stay dual-mono; 0.5 is the widest.
pub fn morph_side(a: f32, b: f32, morph: f32, width: f32) -> f32 {
    let m = morph.clamp(0.0, 1.0);
    0.5 * (a - b) * width.clamp(0.0, 1.0) * 4.0 * m * (1.0 - m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_and_clip_gains_are_independent() {
        let mut clips = [1.0f32; MAX_CLIPS];
        clips[0] = 0.0;
        clips[16] = 0.25;
        assert_eq!(MixSource::Live.gain(0.8, &clips), 0.8);
        assert_eq!(MixSource::clip(0).gain(0.8, &clips), 0.0);
        assert_eq!(MixSource::clip(16).gain(0.8, &clips), 0.25);
    }

    #[test]
    fn seq_notes_split_across_family_buses() {
        assert_eq!(
            MixSource::seq_family(16, crate::DRUM_CHANNEL),
            MixSource::clip(17)
        );
        assert_eq!(MixSource::seq_family(16, 0), MixSource::clip(16));
        assert_eq!(MixSource::seq_kaoss(16), MixSource::clip(18));
        assert_eq!(MixSource::seq_family(3, crate::DRUM_CHANNEL), MixSource::clip(3));
    }

    #[test]
    fn sweep_gains_are_unity_at_center_and_mute_the_far_side() {
        let (l, r) = sweep_gains(0.5);
        assert!((l - 1.0).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
        let (l, r) = sweep_gains(0.0);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
        let (l, r) = sweep_gains(1.0);
        assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
        let (l, r) = sweep_gains(0.25);
        assert!((l - 1.0).abs() < 1e-6 && (r - 0.5).abs() < 1e-6);
    }

    #[test]
    fn note_sweep_is_center_at_c4() {
        assert!((note_sweep(60) - 0.5).abs() < 1e-6);
        assert!(note_sweep(36) < 0.2);
        assert!(note_sweep(84) > 0.8);
        let (l, r) = width_gains(60, 1.0);
        assert!((l - 1.0).abs() < 1e-5 && (r - 1.0).abs() < 1e-5);
        let (low_l, low_r) = width_gains(36, 1.0);
        assert!(low_l > low_r + 0.2);
        let (high_l, high_r) = width_gains(84, 1.0);
        assert!(high_r > high_l + 0.2);
        let (l, r) = width_gains(36, 0.0);
        assert!((l - 1.0).abs() < 1e-5 && (r - 1.0).abs() < 1e-5);
    }

    #[test]
    fn morph_side_peaks_at_half_blend() {
        assert!(morph_side(1.0, -1.0, 0.0, 1.0).abs() < 1e-6);
        assert!(morph_side(1.0, -1.0, 1.0, 1.0).abs() < 1e-6);
        let wide = morph_side(1.0, -1.0, 0.5, 1.0);
        assert!((wide - 1.0).abs() < 1e-5);
        assert!(morph_side(1.0, -1.0, 0.5, 0.0).abs() < 1e-6);
    }
}
