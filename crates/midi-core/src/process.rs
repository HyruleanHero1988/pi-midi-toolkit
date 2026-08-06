//! Ordered Phase-1 transform chain (channel → CC → velocity).
//!
//! Built once from a preset; after that, [`process_event`] does no heap allocation.

use crate::event::MidiEvent;
use crate::preset::{CcMapEntry, ChannelMapMode, EnginePreset, VelocityConfig};

/// Max events one input can expand into (CC remap is 1:1; reserved for later).
pub const MAX_OUT: usize = 4;
/// Max CC remap rules stored in the hot-path chain (overflow truncated with a warning at load).
pub const MAX_CC_MAP: usize = 64;

/// Stack-allocated output from [`process_event`] — no heap on the hot path.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    events: [MidiEvent; MAX_OUT],
    len: usize,
}

impl ProcessOutput {
    #[inline]
    pub const fn empty() -> Self {
        Self {
            events: [MidiEvent::ChannelPressure {
                channel: 0,
                pressure: 0,
            }; MAX_OUT],
            len: 0,
        }
    }

    #[inline]
    pub fn one(event: MidiEvent) -> Self {
        let mut out = Self::empty();
        out.push(event);
        out
    }

    #[inline]
    pub fn push(&mut self, event: MidiEvent) {
        if self.len < MAX_OUT {
            self.events[self.len] = event;
            self.len += 1;
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn as_slice(&self) -> &[MidiEvent] {
        &self.events[..self.len]
    }

    pub fn iter(&self) -> impl Iterator<Item = MidiEvent> + '_ {
        self.as_slice().iter().copied()
    }
}

/// Hot-path velocity transform (curve stored inline, no `Vec` indirection).
#[derive(Debug, Clone)]
pub enum VelocityRuntime {
    PassThrough,
    AlwaysFull,
    Clamp { floor: u8, ceiling: u8 },
    Curve([u8; 128]),
}

impl VelocityRuntime {
    pub fn from_config(cfg: &VelocityConfig) -> Self {
        match cfg {
            VelocityConfig::PassThrough => Self::PassThrough,
            VelocityConfig::AlwaysFull => Self::AlwaysFull,
            VelocityConfig::Clamp { floor, ceiling } => Self::Clamp {
                floor: *floor,
                ceiling: *ceiling,
            },
            VelocityConfig::Curve { table } => {
                let mut curve = [0u8; 128];
                for (i, slot) in curve.iter_mut().enumerate() {
                    *slot = table.get(i).copied().unwrap_or(i as u8) & 0x7f;
                }
                Self::Curve(curve)
            }
        }
    }

    #[inline]
    pub fn map(&self, velocity: u8) -> u8 {
        let v = velocity.min(127);
        match self {
            Self::PassThrough => v,
            Self::AlwaysFull => {
                if v == 0 {
                    0
                } else {
                    127
                }
            }
            Self::Clamp { floor, ceiling } => {
                if v == 0 {
                    0
                } else {
                    v.clamp(*floor, (*ceiling).min(127))
                }
            }
            Self::Curve(table) => table[v as usize] & 0x7f,
        }
    }
}

/// Fixed processor chain built from a preset (owned copy for RT use).
#[derive(Debug, Clone)]
pub struct ProcessorChain {
    channel_map: ChannelMapMode,
    cc_map: [CcMapEntry; MAX_CC_MAP],
    cc_len: usize,
    velocity: VelocityRuntime,
}

impl ProcessorChain {
    pub fn from_preset(preset: &EnginePreset) -> Self {
        let mut cc_map = [CcMapEntry {
            in_channel: 0,
            in_cc: 0,
            out_channel: 0,
            out_cc: 0,
        }; MAX_CC_MAP];
        let n = preset.cc_map.len().min(MAX_CC_MAP);
        cc_map[..n].clone_from_slice(&preset.cc_map[..n]);
        Self {
            channel_map: preset.channel_map.clone(),
            cc_map,
            cc_len: n,
            velocity: VelocityRuntime::from_config(&preset.velocity),
        }
    }

    pub fn identity() -> Self {
        Self {
            channel_map: ChannelMapMode::Identity,
            cc_map: [CcMapEntry {
                in_channel: 0,
                in_cc: 0,
                out_channel: 0,
                out_cc: 0,
            }; MAX_CC_MAP],
            cc_len: 0,
            velocity: VelocityRuntime::PassThrough,
        }
    }

    #[inline]
    pub fn channel_map(&self) -> &ChannelMapMode {
        &self.channel_map
    }

    #[inline]
    pub fn velocity(&self) -> &VelocityRuntime {
        &self.velocity
    }

    #[inline]
    pub fn cc_map(&self) -> &[CcMapEntry] {
        &self.cc_map[..self.cc_len]
    }
}

/// Apply channel remap → CC remap → velocity. Returns 0 or 1 events today.
pub fn process_event(chain: &ProcessorChain, event: MidiEvent) -> ProcessOutput {
    let event = match event {
        MidiEvent::ControlChange {
            channel,
            controller,
            value,
        } => {
            if let Some(mapped) = lookup_cc(chain.cc_map(), channel, controller) {
                MidiEvent::ControlChange {
                    channel: mapped.out_channel & 0x0f,
                    controller: mapped.out_cc & 0x7f,
                    value,
                }
            } else {
                event.with_channel(chain.channel_map.map_channel(channel))
            }
        }
        other => {
            let ch = chain.channel_map.map_channel(other.channel());
            let other = other.with_channel(ch);
            match other {
                MidiEvent::NoteOn {
                    channel,
                    note,
                    velocity,
                } => MidiEvent::NoteOn {
                    channel,
                    note,
                    velocity: chain.velocity.map(velocity),
                },
                e => e,
            }
        }
    };
    ProcessOutput::one(event)
}

fn lookup_cc(map: &[CcMapEntry], channel: u8, controller: u8) -> Option<&CcMapEntry> {
    let channel = channel & 0x0f;
    let controller = controller & 0x7f;
    map.iter()
        .find(|e| (e.in_channel & 0x0f) == channel && (e.in_cc & 0x7f) == controller)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::{CcMapEntry, ChannelMapMode, PortsConfig, VelocityConfig};

    fn chain(
        channel_map: ChannelMapMode,
        cc_map: Vec<CcMapEntry>,
        velocity: VelocityConfig,
    ) -> ProcessorChain {
        ProcessorChain::from_preset(&EnginePreset {
            name: "test".into(),
            ports: PortsConfig {
                input: "in".into(),
                output: "out".into(),
            },
            channel_map,
            cc_map,
            velocity,
        })
    }

    #[test]
    fn channel_all_to() {
        let c = chain(
            ChannelMapMode::AllTo { channel: 2 },
            vec![],
            VelocityConfig::PassThrough,
        );
        let out = process_event(
            &c,
            MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 80,
            },
        );
        assert_eq!(
            out.as_slice(),
            &[MidiEvent::NoteOn {
                channel: 2,
                note: 60,
                velocity: 80,
            }]
        );
    }

    #[test]
    fn channel_remap_table() {
        let mut map = [0u8; 16];
        map[0] = 2;
        map[1] = 3;
        let c = chain(
            ChannelMapMode::Remap { map },
            vec![],
            VelocityConfig::PassThrough,
        );
        assert_eq!(
            process_event(
                &c,
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 10,
                    velocity: 1,
                }
            )
            .as_slice()[0]
            .channel(),
            2
        );
        assert_eq!(
            process_event(
                &c,
                MidiEvent::NoteOn {
                    channel: 1,
                    note: 10,
                    velocity: 1,
                }
            )
            .as_slice()[0]
            .channel(),
            3
        );
    }

    #[test]
    fn always_full_velocity() {
        let c = chain(
            ChannelMapMode::Identity,
            vec![],
            VelocityConfig::AlwaysFull,
        );
        let out = process_event(
            &c,
            MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 40,
            },
        );
        assert_eq!(
            out.as_slice()[0],
            MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 127,
            }
        );
        let off = process_event(
            &c,
            MidiEvent::NoteOff {
                channel: 0,
                note: 60,
                velocity: 64,
            },
        );
        assert_eq!(
            off.as_slice()[0],
            MidiEvent::NoteOff {
                channel: 0,
                note: 60,
                velocity: 64,
            }
        );
    }

    #[test]
    fn velocity_curve_runtime() {
        let mut table = vec![0u8; 128];
        table[40] = 90;
        let c = chain(
            ChannelMapMode::Identity,
            vec![],
            VelocityConfig::Curve { table },
        );
        assert_eq!(
            process_event(
                &c,
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 1,
                    velocity: 40,
                }
            )
            .as_slice()[0],
            MidiEvent::NoteOn {
                channel: 0,
                note: 1,
                velocity: 90,
            }
        );
    }

    #[test]
    fn cc_remap_overrides_channel_map() {
        let c = chain(
            ChannelMapMode::AllTo { channel: 5 },
            vec![CcMapEntry {
                in_channel: 0,
                in_cc: 1,
                out_channel: 2,
                out_cc: 11,
            }],
            VelocityConfig::PassThrough,
        );
        let out = process_event(
            &c,
            MidiEvent::ControlChange {
                channel: 0,
                controller: 1,
                value: 90,
            },
        );
        assert_eq!(
            out.as_slice()[0],
            MidiEvent::ControlChange {
                channel: 2,
                controller: 11,
                value: 90,
            }
        );
    }

    #[test]
    fn unmatched_cc_gets_channel_map() {
        let c = chain(
            ChannelMapMode::AllTo { channel: 3 },
            vec![],
            VelocityConfig::PassThrough,
        );
        let out = process_event(
            &c,
            MidiEvent::ControlChange {
                channel: 0,
                controller: 7,
                value: 100,
            },
        );
        assert_eq!(
            out.as_slice()[0],
            MidiEvent::ControlChange {
                channel: 3,
                controller: 7,
                value: 100,
            }
        );
    }

    #[test]
    fn velocity_clamp() {
        let c = chain(
            ChannelMapMode::Identity,
            vec![],
            VelocityConfig::Clamp {
                floor: 40,
                ceiling: 100,
            },
        );
        assert_eq!(
            process_event(
                &c,
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 1,
                    velocity: 10,
                }
            )
            .as_slice()[0],
            MidiEvent::NoteOn {
                channel: 0,
                note: 1,
                velocity: 40,
            }
        );
    }

    #[test]
    fn process_event_is_allocation_free_smoke() {
        let c = chain(
            ChannelMapMode::AllTo { channel: 2 },
            vec![CcMapEntry {
                in_channel: 0,
                in_cc: 1,
                out_channel: 2,
                out_cc: 11,
            }],
            VelocityConfig::AlwaysFull,
        );
        for i in 0..10_000u32 {
            let _ = process_event(
                &c,
                MidiEvent::NoteOn {
                    channel: 0,
                    note: (i % 128) as u8,
                    velocity: 40,
                },
            );
        }
    }
}
