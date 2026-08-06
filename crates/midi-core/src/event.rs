//! MIDI event types and wire encode/decode (status + data bytes only).

/// MIDI channel 0–15 (wire channel 1–16).
pub type Channel = u8;
/// MIDI note number 0–127.
pub type Note = u8;
/// MIDI velocity 0–127.
pub type Velocity = u8;

/// Channel voice / common messages we transform on the thru path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiEvent {
    NoteOff {
        channel: Channel,
        note: Note,
        velocity: Velocity,
    },
    NoteOn {
        channel: Channel,
        note: Note,
        velocity: Velocity,
    },
    PolyPressure {
        channel: Channel,
        note: Note,
        pressure: u8,
    },
    ControlChange {
        channel: Channel,
        controller: u8,
        value: u8,
    },
    ProgramChange {
        channel: Channel,
        program: u8,
    },
    ChannelPressure {
        channel: Channel,
        pressure: u8,
    },
    PitchBend {
        channel: Channel,
        /// 14-bit value, 0–16383 (8192 = center).
        value: u16,
    },
}

impl MidiEvent {
    #[inline]
    pub fn channel(self) -> Channel {
        match self {
            Self::NoteOff { channel, .. }
            | Self::NoteOn { channel, .. }
            | Self::PolyPressure { channel, .. }
            | Self::ControlChange { channel, .. }
            | Self::ProgramChange { channel, .. }
            | Self::ChannelPressure { channel, .. }
            | Self::PitchBend { channel, .. } => channel,
        }
    }

    #[inline]
    pub fn with_channel(self, channel: Channel) -> Self {
        let channel = channel & 0x0f;
        match self {
            Self::NoteOff { note, velocity, .. } => Self::NoteOff {
                channel,
                note,
                velocity,
            },
            Self::NoteOn { note, velocity, .. } => Self::NoteOn {
                channel,
                note,
                velocity,
            },
            Self::PolyPressure { note, pressure, .. } => Self::PolyPressure {
                channel,
                note,
                pressure,
            },
            Self::ControlChange {
                controller, value, ..
            } => Self::ControlChange {
                channel,
                controller,
                value,
            },
            Self::ProgramChange { program, .. } => Self::ProgramChange { channel, program },
            Self::ChannelPressure { pressure, .. } => Self::ChannelPressure { channel, pressure },
            Self::PitchBend { value, .. } => Self::PitchBend { channel, value },
        }
    }

    /// Parse a short MIDI message (1–3 bytes). SysEx and realtime are ignored.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        let status = bytes[0];
        if status < 0x80 || status >= 0xf0 {
            return None;
        }
        let channel = status & 0x0f;
        let kind = status & 0xf0;
        match kind {
            0x80 if bytes.len() >= 3 => Some(Self::NoteOff {
                channel,
                note: bytes[1] & 0x7f,
                velocity: bytes[2] & 0x7f,
            }),
            0x90 if bytes.len() >= 3 => {
                let note = bytes[1] & 0x7f;
                let velocity = bytes[2] & 0x7f;
                if velocity == 0 {
                    Some(Self::NoteOff {
                        channel,
                        note,
                        velocity: 0,
                    })
                } else {
                    Some(Self::NoteOn {
                        channel,
                        note,
                        velocity,
                    })
                }
            }
            0xa0 if bytes.len() >= 3 => Some(Self::PolyPressure {
                channel,
                note: bytes[1] & 0x7f,
                pressure: bytes[2] & 0x7f,
            }),
            0xb0 if bytes.len() >= 3 => Some(Self::ControlChange {
                channel,
                controller: bytes[1] & 0x7f,
                value: bytes[2] & 0x7f,
            }),
            0xc0 if bytes.len() >= 2 => Some(Self::ProgramChange {
                channel,
                program: bytes[1] & 0x7f,
            }),
            0xd0 if bytes.len() >= 2 => Some(Self::ChannelPressure {
                channel,
                pressure: bytes[1] & 0x7f,
            }),
            0xe0 if bytes.len() >= 3 => {
                let lsb = (bytes[1] & 0x7f) as u16;
                let msb = (bytes[2] & 0x7f) as u16;
                Some(Self::PitchBend {
                    channel,
                    value: lsb | (msb << 7),
                })
            }
            _ => None,
        }
    }

    /// Encode into `buf`. Returns the number of bytes written (1–3).
    pub fn encode(self, buf: &mut [u8; 3]) -> usize {
        match self {
            Self::NoteOff {
                channel,
                note,
                velocity,
            } => {
                buf[0] = 0x80 | (channel & 0x0f);
                buf[1] = note & 0x7f;
                buf[2] = velocity & 0x7f;
                3
            }
            Self::NoteOn {
                channel,
                note,
                velocity,
            } => {
                buf[0] = 0x90 | (channel & 0x0f);
                buf[1] = note & 0x7f;
                buf[2] = velocity & 0x7f;
                3
            }
            Self::PolyPressure {
                channel,
                note,
                pressure,
            } => {
                buf[0] = 0xa0 | (channel & 0x0f);
                buf[1] = note & 0x7f;
                buf[2] = pressure & 0x7f;
                3
            }
            Self::ControlChange {
                channel,
                controller,
                value,
            } => {
                buf[0] = 0xb0 | (channel & 0x0f);
                buf[1] = controller & 0x7f;
                buf[2] = value & 0x7f;
                3
            }
            Self::ProgramChange { channel, program } => {
                buf[0] = 0xc0 | (channel & 0x0f);
                buf[1] = program & 0x7f;
                2
            }
            Self::ChannelPressure { channel, pressure } => {
                buf[0] = 0xd0 | (channel & 0x0f);
                buf[1] = pressure & 0x7f;
                2
            }
            Self::PitchBend { channel, value } => {
                let value = value.min(16383);
                buf[0] = 0xe0 | (channel & 0x0f);
                buf[1] = (value & 0x7f) as u8;
                buf[2] = ((value >> 7) & 0x7f) as u8;
                3
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_note_on() {
        let ev = MidiEvent::NoteOn {
            channel: 2,
            note: 60,
            velocity: 100,
        };
        let mut buf = [0u8; 3];
        let n = ev.encode(&mut buf);
        assert_eq!(n, 3);
        assert_eq!(MidiEvent::parse(&buf[..n]), Some(ev));
    }

    #[test]
    fn note_on_vel_zero_is_note_off() {
        assert_eq!(
            MidiEvent::parse(&[0x90, 60, 0]),
            Some(MidiEvent::NoteOff {
                channel: 0,
                note: 60,
                velocity: 0,
            })
        );
    }

    #[test]
    fn pitch_bend_center() {
        let ev = MidiEvent::PitchBend {
            channel: 0,
            value: 8192,
        };
        let mut buf = [0u8; 3];
        let n = ev.encode(&mut buf);
        assert_eq!(MidiEvent::parse(&buf[..n]), Some(ev));
    }
}
