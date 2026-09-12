//! Live vs clip mix sources.
//!
//! MIX mode trims these independently: live keys, live kit, and each clip slot
//! (phrase pads + SEQ) so a running sequence can sit under a live take.

use crate::clip::MAX_CLIPS;

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
}
