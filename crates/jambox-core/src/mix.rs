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
}
