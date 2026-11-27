//! MIDI in/out threads.
//!
//! Input: hardware ports are watched and hot-plugged. Bytes (and UI-injected
//! MIDI on the control socket) are parsed once, turned into [`Command`]s, and
//! fanned out to the kiosk. Knob meaning lives here — Tk only displays.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use midi_core::{
    fanout_dest_mask, is_virtual_port_name, matching_port_names, pick_thru_output_name,
    profile_for_port_name, ControllerProfile, KnobEncoding, MidiDest, MidiEvent, MidiSource,
};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use tracing::{info, warn};

use crate::bus::{MidiInSide, MidiOutSide};
use jambox_core::{
    drum_model_for_note, hw_repeat_owner, Command, FxParam, FxTarget, RepeatDivision, SynthParam,
    DRUM_CHANNEL, DRUM_MODEL_COUNT, SEQ_CLIP_SLOT, SEQ_DRUM_MIX_SLOT, SEQ_KAOSS_MIX_SLOT,
};

/// How often the output thread checks the ring once a port is open.
/// A 0.5 ms poll re-listed every ALSA MIDI port each wake and pegged a core.
const OUT_POLL: Duration = Duration::from_millis(2);
/// How often we look for a newly plugged controller.
const HOTPLUG_POLL: Duration = Duration::from_millis(400);

/// MPK mini factory knobs (Prog Select → Pad 1 / MPC program).
/// K1–K8 are CC 70–77 in order. The names below are the drum/FM/FX bindings.
/// Keys mode maps the same eight CCs onto the synth sliders left to right.
pub const CC_MOD: u8 = 1;
pub const CC_MORPH: u8 = 70;
pub const CC_TONE: u8 = 71;
pub const CC_ATTACK: u8 = 72;
pub const CC_RELEASE: u8 = 73;
pub const CC_VIB_DEPTH: u8 = 74;
pub const CC_VIB_RATE: u8 = 75;
pub const CC_LEVEL: u8 = 77;

pub const MODE_KEYS: u8 = 0;
pub const MODE_DRUMS: u8 = 1;
pub const MODE_FX: u8 = 2;
pub const MODE_FM: u8 = 3;
pub const MODE_PUNCH: u8 = 4;

pub const FX_VOICE: u8 = 0;
pub const FX_DRUM: u8 = 1;
pub const FX_DRUM_GROUP: u8 = 2;
pub const FX_BUS: u8 = 3;

#[derive(Debug, thiserror::Error)]
pub enum MidiError {
    #[error("midi init failed: {0}")]
    Init(String),
    #[error("no MIDI port matched {0:?}")]
    NoPort(String),
    #[error("connect failed: {0}")]
    Connect(String),
}

/// Live knob-bank state. UI buttons write this; MIDI ingest reads it.
pub struct MidiMap {
    mode: AtomicU8,
    fx_kind: AtomicU8,
    fx_index: AtomicU16,
    /// Incoming NoteOns → 127 (MPK velocity workaround). Default on.
    full_vel: AtomicBool,
    /// Scale mode: snap keybed notes into `scale_mask` above `scale_root`.
    scale_on: AtomicBool,
    scale_root: AtomicU8,
    scale_mask: AtomicU16,
    /// Snapped note per (channel, input note) so a note-off always releases
    /// what its note-on played, even if the scale changed in between.
    /// `0xff` = nothing held.
    scale_held: [AtomicU8; 16 * 128],
    /// Joystick X / pitch-wheel destination.
    stick_x: AtomicU8,
    /// Joystick Y / CC1 destination.
    stick_y: AtomicU8,
    /// Pad poly aftertouch destination.
    aftertouch: AtomicU8,
    /// FX page: Bank A pad notes are punch toggles instead of kit hits.
    pads_to_fx: AtomicBool,
    /// Per kit-voice note-repeat slot (0 = off). Hardware pads read this.
    repeat_div: [AtomicU8; DRUM_MODEL_COUNT],
    repeat_latch: AtomicBool,
    hw_latched: [AtomicBool; DRUM_MODEL_COUNT],
    hw_note: [AtomicU8; DRUM_MODEL_COUNT],
    /// Last drum-channel pad note (36..51). Drums-mode knobs, stick, and
    /// channel-pressure drum destinations write that voice only — never the
    /// whole kit — so raising snare LEVEL cannot stamp kick/clap/etc.
    /// `0xff` = no pad has spoken yet this session.
    last_drum_note: AtomicU8,
    /// Last absolute 0..=10000 for relative encoders (knobs 0..7, bank2 8..15).
    cc_last: [AtomicU16; 16],
}

impl Default for MidiMap {
    fn default() -> Self {
        Self {
            mode: AtomicU8::new(0),
            fx_kind: AtomicU8::new(0),
            fx_index: AtomicU16::new(0),
            full_vel: AtomicBool::new(true),
            scale_on: AtomicBool::new(false),
            scale_root: AtomicU8::new(0),
            scale_mask: AtomicU16::new(jambox_protocol::default_scale_mask()),
            scale_held: std::array::from_fn(|_| AtomicU8::new(0xff)),
            stick_x: AtomicU8::new(MidiDest::default_for(MidiSource::StickX).as_u8()),
            stick_y: AtomicU8::new(MidiDest::default_for(MidiSource::StickY).as_u8()),
            aftertouch: AtomicU8::new(MidiDest::default_for(MidiSource::Aftertouch).as_u8()),
            pads_to_fx: AtomicBool::new(true),
            repeat_div: std::array::from_fn(|_| AtomicU8::new(0)),
            repeat_latch: AtomicBool::new(false),
            hw_latched: std::array::from_fn(|_| AtomicBool::new(false)),
            hw_note: std::array::from_fn(|_| AtomicU8::new(0)),
            last_drum_note: AtomicU8::new(0xff),
            cc_last: std::array::from_fn(|_| AtomicU16::new(0)),
        }
    }
}

impl MidiMap {
    pub fn set_keys(&self) {
        self.mode.store(MODE_KEYS, Ordering::Relaxed);
    }

    pub fn set_drums(&self) {
        self.mode.store(MODE_DRUMS, Ordering::Relaxed);
    }

    pub fn set_fx(&self, kind: u8, index: u16) {
        self.mode.store(MODE_FX, Ordering::Relaxed);
        self.fx_kind.store(kind, Ordering::Relaxed);
        self.fx_index.store(index, Ordering::Relaxed);
    }

    pub fn set_fm(&self) {
        self.mode.store(MODE_FM, Ordering::Relaxed);
    }

    pub fn set_punch(&self) {
        self.mode.store(MODE_PUNCH, Ordering::Relaxed);
    }

    pub fn set_full_vel(&self, on: bool) {
        self.full_vel.store(on, Ordering::Relaxed);
    }

    /// Scale mode on/off. An empty mask is treated as chromatic (no-op).
    pub fn set_scale_lock(&self, on: bool, root: u8, mask: u16) {
        self.scale_root.store(root % 12, Ordering::Relaxed);
        self.scale_mask.store(mask & 0x0fff, Ordering::Relaxed);
        self.scale_on.store(on, Ordering::Relaxed);
    }

    pub fn scale_lock(&self) -> (bool, u8, u16) {
        (
            self.scale_on.load(Ordering::Relaxed),
            self.scale_root.load(Ordering::Relaxed),
            self.scale_mask.load(Ordering::Relaxed),
        )
    }

    /// Scale mode: remap keybed notes onto unique scale degrees. Drum channel
    /// and the controller's transport notes pass through untouched.
    pub fn apply_scale(&self, event: MidiEvent, profile: &ControllerProfile) -> MidiEvent {
        let (on, root, mask) = self.scale_lock();
        let lookup =
            |channel: u8, note: u8| (channel as usize & 0x0f) * 128 + (note as usize & 0x7f);
        match event {
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } if velocity > 0
                && on
                && channel != DRUM_CHANNEL
                && !profile.is_transport_note(note) =>
            {
                let snapped = snap_to_scale(note, root, mask);
                self.scale_held[lookup(channel, note)].store(snapped, Ordering::Relaxed);
                MidiEvent::NoteOn {
                    channel,
                    note: snapped,
                    velocity,
                }
            }
            MidiEvent::NoteOn {
                channel,
                note,
                velocity: 0,
            } => {
                let held = self.scale_held[lookup(channel, note)].swap(0xff, Ordering::Relaxed);
                let note = if held == 0xff { note } else { held };
                MidiEvent::NoteOn {
                    channel,
                    note,
                    velocity: 0,
                }
            }
            MidiEvent::NoteOff {
                channel,
                note,
                velocity,
            } => {
                let held = self.scale_held[lookup(channel, note)].swap(0xff, Ordering::Relaxed);
                let note = if held == 0xff { note } else { held };
                MidiEvent::NoteOff {
                    channel,
                    note,
                    velocity,
                }
            }
            MidiEvent::PolyPressure {
                channel,
                note,
                pressure,
            } => {
                let held = self.scale_held[lookup(channel, note)].load(Ordering::Relaxed);
                let note = if held == 0xff { note } else { held };
                MidiEvent::PolyPressure {
                    channel,
                    note,
                    pressure,
                }
            }
            other => other,
        }
    }

    pub fn set_assigns(
        &self,
        stick_x: Option<&str>,
        stick_y: Option<&str>,
        aftertouch: Option<&str>,
    ) {
        if let Some(dest) = stick_x.and_then(MidiDest::from_id) {
            self.stick_x.store(dest.as_u8(), Ordering::Relaxed);
        }
        if let Some(dest) = stick_y.and_then(MidiDest::from_id) {
            self.stick_y.store(dest.as_u8(), Ordering::Relaxed);
        }
        if let Some(dest) = aftertouch.and_then(MidiDest::from_id) {
            self.aftertouch.store(dest.as_u8(), Ordering::Relaxed);
        }
    }

    fn dest_for(&self, source: MidiSource) -> MidiDest {
        let raw = match source {
            MidiSource::StickX => self.stick_x.load(Ordering::Relaxed),
            MidiSource::StickY => self.stick_y.load(Ordering::Relaxed),
            MidiSource::Aftertouch => self.aftertouch.load(Ordering::Relaxed),
        };
        MidiDest::from_u8(raw)
    }

    pub fn set_pads_to_fx(&self, armed: bool) {
        self.pads_to_fx.store(armed, Ordering::Relaxed);
    }

    fn remember_drum_note(&self, note: u8) {
        self.last_drum_note.store(note, Ordering::Relaxed);
    }

    fn last_drum_note(&self) -> Option<u8> {
        let note = self.last_drum_note.load(Ordering::Relaxed);
        if note == 0xff {
            None
        } else {
            Some(note)
        }
    }

    fn pads_to_fx(&self) -> bool {
        self.pads_to_fx.load(Ordering::Relaxed)
    }

    pub fn full_vel(&self) -> bool {
        self.full_vel.load(Ordering::Relaxed)
    }

    /// Rewrite keyboard note-ons to 127 when FULL VEL is on. Drums / note-offs stay.
    /// Mirror UI latched repeats so hardware re-tap toggles the same lane.
    pub fn set_hw_repeat_latched(&self, note: u8, latched: bool) {
        let model = drum_model_for_note(note).index();
        if latched {
            self.hw_note[model].store(note, Ordering::Relaxed);
            self.hw_latched[model].store(true, Ordering::Relaxed);
        } else {
            self.hw_latched[model].store(false, Ordering::Relaxed);
        }
    }

    pub fn set_drum_repeat(&self, slots: [u8; 16], latch: bool) -> [u32; DRUM_MODEL_COUNT] {
        let was_latch = self.repeat_latch.swap(latch, Ordering::Relaxed);
        let mut stops = [0u32; DRUM_MODEL_COUNT];
        for i in 0..DRUM_MODEL_COUNT {
            let slot = slots.get(i).copied().unwrap_or(0);
            self.repeat_div[i].store(slot, Ordering::Relaxed);
            let drop = slot == 0 || (was_latch && !latch);
            if drop && self.hw_latched[i].swap(false, Ordering::Relaxed) {
                let note = self.hw_note[i].load(Ordering::Relaxed);
                stops[i] = hw_repeat_owner(note);
            }
        }
        stops
    }

    fn interpret_drum_repeat(&self, note: u8, velocity: u8, on: bool) -> Option<Command> {
        let division = self.repeat_division_for_note(note)?;
        let model = drum_model_for_note(note).index();
        let latch = self.repeat_latch.load(Ordering::Relaxed);
        if on {
            if latch && self.hw_latched[model].swap(false, Ordering::Relaxed) {
                let prev = self.hw_note[model].load(Ordering::Relaxed);
                return Some(Command::StopRepeat {
                    owner: hw_repeat_owner(prev),
                });
            }
            self.hw_note[model].store(note, Ordering::Relaxed);
            self.hw_latched[model].store(latch, Ordering::Relaxed);
            Some(Command::StartRepeat {
                owner: hw_repeat_owner(note),
                channel: DRUM_CHANNEL,
                note,
                velocity: velocity.max(1),
                division,
            })
        } else if latch {
            None
        } else {
            self.hw_latched[model].store(false, Ordering::Relaxed);
            Some(Command::StopRepeat {
                owner: hw_repeat_owner(note),
            })
        }
    }

    fn repeat_division_for_note(&self, note: u8) -> Option<RepeatDivision> {
        let slot = self.repeat_div[drum_model_for_note(note).index()].load(Ordering::Relaxed);
        match slot {
            1 => Some(RepeatDivision::Quarter),
            2 => Some(RepeatDivision::Eighth),
            3 => Some(RepeatDivision::EighthTriplet),
            4 => Some(RepeatDivision::Sixteenth),
            5 => Some(RepeatDivision::Eighth), // legacy “triple” slot
            6 => Some(RepeatDivision::Half),
            7 => Some(RepeatDivision::Whole),
            _ => None,
        }
    }

    pub fn apply_velocity(&self, event: MidiEvent) -> MidiEvent {
        if !self.full_vel() {
            return event;
        }
        match event {
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } if velocity > 0 => MidiEvent::NoteOn {
                channel,
                note,
                velocity: 127,
            },
            other => other,
        }
    }

    pub fn apply_knob_map(&self, mode: &str, fx_kind: Option<&str>, fx_index: u16) {
        match mode {
            "drums" => self.set_drums(),
            "fm" => self.set_fm(),
            "punch" => self.set_punch(),
            "fx" => {
                let kind = match fx_kind.unwrap_or("voice") {
                    "drum" => FX_DRUM,
                    "drums" | "drum_group" => FX_DRUM_GROUP,
                    "bus" => FX_BUS,
                    _ => FX_VOICE,
                };
                self.set_fx(kind, fx_index);
            }
            _ => self.set_keys(),
        }
    }

    fn fx_target(&self) -> FxTarget {
        match self.fx_kind.load(Ordering::Relaxed) {
            FX_DRUM => FxTarget::Drum(self.fx_index.load(Ordering::Relaxed) as u8),
            FX_DRUM_GROUP => FxTarget::DrumGroup,
            FX_BUS => FxTarget::Bus,
            _ => FxTarget::Voice(self.fx_index.load(Ordering::Relaxed)),
        }
    }

    /// One MIDI event → zero or more audio-thread commands (MPK factory map).
    pub fn interpret(&self, event: MidiEvent) -> Option<Command> {
        let src = event.channel();
        self.interpret_with(&ControllerProfile::mpk_mini(), event, src)
    }

    /// Voice channel for the engine: drums only when the **incoming** wire
    /// channel is GM drums (index 9). MAP fan-out may rewrite `event.channel()`
    /// for synth allocation; `src_channel` is the pre-map source.
    #[inline]
    fn engine_play_channel(src_channel: u8, mapped_channel: u8) -> u8 {
        if src_channel == DRUM_CHANNEL {
            DRUM_CHANNEL
        } else if mapped_channel == DRUM_CHANNEL {
            // MAP may fan ch1→ch10 for external gear; internal kit only
            // follows the wire channel, so this stays on the key channel.
            src_channel
        } else {
            mapped_channel
        }
    }

    /// One MIDI event through a live controller map. Unknown devices use
    /// [`ControllerProfile::generic`] (MPK-compatible knobs/pads).
    pub fn interpret_with(
        &self,
        profile: &ControllerProfile,
        event: MidiEvent,
        src_channel: u8,
    ) -> Option<Command> {
        match event {
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } => {
                if profile.is_transport_note(note) {
                    return None;
                }
                if src_channel == DRUM_CHANNEL {
                    self.remember_drum_note(note);
                }
                // Pads→FX steals profile pad notes (pad channel or GM ch10).
                // Keybed pitches that reuse those numbers on another channel
                // (classic MPK: pads ch10, keys ch1) still sound on FX.
                if self.mode.load(Ordering::Relaxed) == MODE_PUNCH
                    && self.pads_to_fx()
                    && profile.is_fx_pad_hit(channel, note)
                {
                    return None;
                }
                let play_ch = Self::engine_play_channel(src_channel, channel);
                if play_ch == DRUM_CHANNEL {
                    if let Some(command) = self.interpret_drum_repeat(note, velocity, true) {
                        return Some(command);
                    }
                }
                Some(Command::NoteOn {
                    channel: play_ch,
                    note,
                    velocity,
                })
            }
            MidiEvent::NoteOff { channel, note, .. } => {
                if profile.is_transport_note(note) {
                    return None;
                }
                if self.mode.load(Ordering::Relaxed) == MODE_PUNCH
                    && self.pads_to_fx()
                    && profile.is_fx_pad_hit(channel, note)
                {
                    return None;
                }
                let play_ch = Self::engine_play_channel(src_channel, channel);
                if play_ch == DRUM_CHANNEL && self.repeat_division_for_note(note).is_some() {
                    return self.interpret_drum_repeat(note, 0, false);
                }
                Some(Command::NoteOff {
                    channel: play_ch,
                    note,
                })
            }
            MidiEvent::PitchBend { value, .. } => {
                let dest = self.dest_for(MidiSource::StickX);
                let unit = (value as f32 / 16_383.0).clamp(0.0, 1.0);
                let semis = ((value as f32) - 8192.0) / 8192.0 * 2.0;
                // Drum destinations follow the last pad so stick X cannot
                // rewrite every voice's pitch/level at once.
                dest_command(dest, unit, Some(semis), self.last_drum_note())
            }
            MidiEvent::ControlChange {
                channel,
                controller,
                value,
            } => self.interpret_cc(profile, channel, controller, value),
            MidiEvent::PolyPressure {
                channel,
                note,
                pressure,
            } => self.interpret_aftertouch(profile, Some((channel, note)), pressure),
            MidiEvent::ChannelPressure { pressure, .. } => {
                // No note on the wire — aim at the last pad so a hard hit
                // cannot raise every drum's stored LEVEL/TONE together.
                let pad = self.last_drum_note().map(|note| (DRUM_CHANNEL, note));
                self.interpret_aftertouch(profile, pad, pressure)
            }
            _ => None,
        }
    }

    fn interpret_aftertouch(
        &self,
        profile: &ControllerProfile,
        pad: Option<(u8, u8)>,
        pressure: u8,
    ) -> Option<Command> {
        if let Some((channel, note)) = pad {
            if profile.is_transport_note(note) {
                return None;
            }
            if self.mode.load(Ordering::Relaxed) == MODE_PUNCH
                && self.pads_to_fx()
                && profile.is_fx_pad_hit(channel, note)
            {
                return None;
            }
            if channel != DRUM_CHANNEL {
                return dest_command(
                    self.dest_for(MidiSource::Aftertouch),
                    (pressure as f32 / 127.0).clamp(0.0, 1.0),
                    None,
                    None,
                );
            }
            dest_command(
                self.dest_for(MidiSource::Aftertouch),
                (pressure as f32 / 127.0).clamp(0.0, 1.0),
                None,
                Some(note),
            )
        } else {
            dest_command(
                self.dest_for(MidiSource::Aftertouch),
                (pressure as f32 / 127.0).clamp(0.0, 1.0),
                None,
                None,
            )
        }
    }

    fn cc_unit(&self, slot: usize, encoding: KnobEncoding, value: u8) -> f32 {
        let last = if slot < self.cc_last.len() {
            self.cc_last[slot].load(Ordering::Relaxed) as f32 / 10_000.0
        } else {
            0.0
        };
        let unit = midi_core::apply_knob_cc(encoding, last, value);
        if slot < self.cc_last.len() {
            self.cc_last[slot].store((unit * 10_000.0).round() as u16, Ordering::Relaxed);
        }
        unit
    }

    fn interpret_cc(
        &self,
        profile: &ControllerProfile,
        channel: u8,
        controller: u8,
        value: u8,
    ) -> Option<Command> {
        if controller == CC_MOD {
            let unit = (value as f32 / 127.0).clamp(0.0, 1.0);
            return dest_command(
                self.dest_for(MidiSource::StickY),
                unit,
                None,
                self.last_drum_note(),
            );
        }
        if controller == 64
            && profile.fader_index(64).is_none()
            && profile.knob_index(64).is_none()
            && profile.knob_bank2_index(64).is_none()
        {
            return Some(Command::SetSynth {
                param: SynthParam::Sustain,
                value: if value >= 64 { 1.0 } else { 0.0 },
            });
        }
        if profile.faders_on_channel(channel) {
            if let Some(index) = profile.fader_index(controller) {
                let unit = (value as f32 / 127.0).clamp(0.0, 1.0);
                return mix_fader_command(index, unit);
            }
        }
        if !profile.knobs_on_channel(channel) {
            return None;
        }
        if let Some(index) = profile.knob_bank2_index(controller) {
            let unit = self.cc_unit(8 + index, profile.knob_encoding, value);
            return bank2_fx_command(index, unit);
        }
        let knob = profile.knob_index(controller)?;
        let unit = self.cc_unit(knob, profile.knob_encoding, value);
        let mode = self.mode.load(Ordering::Relaxed);
        if mode == MODE_PUNCH && profile.punch_index_for_knob(controller).is_some() {
            // Amounts / arming live in the kiosk so a disarmed slide stays dry.
            return None;
        }
        if mode == MODE_FX {
            let param = match knob {
                0 => FxParam::Drive,
                1 => FxParam::DelayTime,
                2 => FxParam::DelayFb,
                3 => FxParam::DelayMix,
                4 => FxParam::ReverbSize,
                5 => FxParam::ReverbMix,
                7 => {
                    return Some(Command::SetSynth {
                        param: SynthParam::Level,
                        value: unit.powf(1.15),
                    });
                }
                _ => return None,
            };
            return Some(Command::SetFx {
                target: self.fx_target(),
                param,
                value: unit,
            });
        }
        if mode == MODE_DRUMS {
            // K1–K4 reshape the last-hit pad only. Kit-wide SetSynth used to
            // make "play loud snare → every drum got louder" when a knob or
            // residual CC landed after a pad hit. K8 stays the shared kit bus.
            let param = match knob {
                0 => SynthParam::DrumPitch,
                1 => SynthParam::DrumTone,
                2 => SynthParam::DrumDecay,
                3 => SynthParam::DrumNoise,
                7 => {
                    return Some(Command::SetSynth {
                        param: SynthParam::DrumLevel,
                        value: unit.powf(1.15),
                    });
                }
                _ => return None,
            };
            return drum_express(param, unit, self.last_drum_note());
        }
        if mode == MODE_FM {
            // Operator / width / glide live in the kiosk (like punch amounts) so
            // hardware knobs move the on-screen FM sliders and session state.
            return None;
        }
        // Bank-1 knobs on the synth page follow the slider row: morph, tone,
        // level, attack, release, drift, flange mix, flange rate. Drum / FM /
        // FX maps above keep their own CCs. CC1 is the joystick and is handled
        // before this match.
        match knob {
            0 => Some(Command::SetSynth {
                param: SynthParam::Morph,
                value: unit,
            }),
            1 => Some(Command::SetSynth {
                param: SynthParam::Tone,
                value: unit,
            }),
            2 => Some(Command::SetSynth {
                param: SynthParam::Level,
                value: unit,
            }),
            3 => Some(Command::SetSynth {
                param: SynthParam::Attack,
                value: unit,
            }),
            4 => Some(Command::SetSynth {
                param: SynthParam::Release,
                value: unit,
            }),
            5 => Some(Command::SetSynth {
                param: SynthParam::ReleaseDrift,
                value: unit,
            }),
            6 => Some(Command::SetFx {
                target: FxTarget::Voice(self.fx_index.load(Ordering::Relaxed)),
                param: FxParam::FlangerMix,
                value: unit,
            }),
            7 => Some(Command::SetFx {
                target: FxTarget::Voice(self.fx_index.load(Ordering::Relaxed)),
                param: FxParam::FlangerRate,
                value: unit,
            }),
            _ => None,
        }
    }
}

fn dest_command(
    dest: MidiDest,
    unit: f32,
    pitch_semis: Option<f32>,
    drum_note: Option<u8>,
) -> Option<Command> {
    let unit = unit.clamp(0.0, 1.0);
    match dest {
        MidiDest::Off => None,
        MidiDest::Vibrato => Some(Command::SetSynth {
            param: SynthParam::VibratoMod,
            value: unit,
        }),
        MidiDest::Pitch => Some(Command::SetSynth {
            param: SynthParam::PitchBend,
            value: pitch_semis.unwrap_or((unit - 0.5) * 4.0),
        }),
        MidiDest::Tone => Some(Command::SetSynth {
            param: SynthParam::Tone,
            value: unit,
        }),
        MidiDest::Morph => Some(Command::SetSynth {
            param: SynthParam::Morph,
            value: unit,
        }),
        MidiDest::Level => Some(Command::SetSynth {
            param: SynthParam::Level,
            value: unit,
        }),
        MidiDest::DrumTone => {
            if let Some(note) = drum_note {
                Some(Command::SetDrumAftertouch {
                    model: drum_model_for_note(note).index() as u8,
                    amount: unit,
                })
            } else {
                // No pad context: refuse kit-wide tone stomps. ALL DRUMS still
                // reaches the engine through the UI SetSynth path.
                None
            }
        }
        MidiDest::DrumPitch => drum_express(SynthParam::DrumPitch, unit, drum_note),
        MidiDest::DrumDecay => drum_express(SynthParam::DrumDecay, unit, drum_note),
        MidiDest::DrumNoise => drum_express(SynthParam::DrumNoise, unit, drum_note),
        MidiDest::DrumLevel => drum_express(SynthParam::DrumAmp, unit, drum_note),
    }
}

fn drum_express(param: SynthParam, unit: f32, drum_note: Option<u8>) -> Option<Command> {
    // Never fall back to kit-wide SetSynth: that is what made one pad's LEVEL
    // appear to raise the whole kit. Hit a pad first (or use ALL DRUMS on UI).
    let note = drum_note?;
    Some(Command::SetDrumMacro {
        model: drum_model_for_note(note).index() as u8,
        param,
        value: unit,
    })
}

fn bank2_fx_command(index: usize, unit: f32) -> Option<Command> {
    let param = match index {
        0 => FxParam::Drive,
        1 => FxParam::DelayTime,
        2 => FxParam::DelayFb,
        3 => FxParam::DelayMix,
        4 => FxParam::ReverbSize,
        5 => FxParam::ReverbMix,
        6 => FxParam::FlangerMix,
        7 => FxParam::FlangerRate,
        _ => return None,
    };
    Some(Command::SetFx {
        target: FxTarget::Bus,
        param,
        value: unit,
    })
}

fn mix_fader_command(index: usize, unit: f32) -> Option<Command> {
    match index {
        0 => Some(Command::SetSynth {
            param: SynthParam::Level,
            value: unit,
        }),
        1 => Some(Command::SetSynth {
            param: SynthParam::DrumLevel,
            value: unit,
        }),
        2 => Some(Command::SetClipGain {
            slot: SEQ_DRUM_MIX_SLOT,
            value: (unit * 2.0).clamp(0.0, 2.0),
        }),
        3 => Some(Command::SetClipGain {
            slot: SEQ_CLIP_SLOT,
            value: (unit * 2.0).clamp(0.0, 2.0),
        }),
        4 => Some(Command::SetClipGain {
            slot: SEQ_KAOSS_MIX_SLOT,
            value: (unit * 2.0).clamp(0.0, 2.0),
        }),
        5 => Some(Command::SetSynth {
            param: SynthParam::InputLevel,
            value: unit,
        }),
        _ => None,
    }
}

/// Remap a chromatic MIDI note onto the scale in `mask` (bit i = `root + i`).
///
/// Each successive physical key walks the next scale degree, so adjacent keys
/// never share a pitch when the scale has fewer than 12 tones. Root keys
/// (`note % 12 == root`) keep their octave (C4 stays C4 in C major). An empty
/// or chromatic mask leaves the note alone.
///
/// Nearest-pitch snapping is intentionally *not* used: it maps several
/// chromatics onto one degree (C and C♯ → C), which sounded like duplicate
/// notes in a row on the keybed.
pub fn snap_to_scale(note: u8, root: u8, mask: u16) -> u8 {
    let mask = mask & 0x0fff;
    if mask == 0 || mask == 0x0fff {
        return note;
    }
    let mut deg = [0u8; 12];
    let mut n_deg = 0usize;
    for i in 0..12u8 {
        if mask & (1 << i) != 0 {
            deg[n_deg] = i;
            n_deg += 1;
        }
    }
    if n_deg == 0 || n_deg == 12 {
        return note;
    }
    let root = (root % 12) as i32;
    let rel = note as i32 - root;
    let kb_oct = rel.div_euclid(12);
    let local = rel.rem_euclid(12);
    let global_idx = kb_oct * n_deg as i32 + local;
    let n = n_deg as i32;
    let oct = global_idx.div_euclid(n);
    let di = global_idx.rem_euclid(n) as usize;
    let out = root + deg[di] as i32 + 12 * oct;
    out.clamp(0, 127) as u8
}

/// Push a MIDI event into DSP and to every UI client except `origin`
/// (the control session that sent it, if it did not come from hardware).
///
/// `dest_mask` is the live channel-map bitmask (`bit j` = emit on channel `j`).
pub fn ingest(
    event: MidiEvent,
    dest_mask: u16,
    origin: Option<u32>,
    hub: &crate::ipc::ClientHub,
    map: &MidiMap,
    profile: &ControllerProfile,
    mut send: impl FnMut(Command) -> bool,
) {
    let event = map.apply_velocity(event);
    let event = map.apply_scale(event, profile);
    let src_channel = event.channel();
    let mask = fanout_dest_mask(dest_mask, src_channel);
    for dest in 0..16u8 {
        if mask & (1 << dest) == 0 {
            continue;
        }
        let mapped = event.with_channel(dest);
        hub.broadcast_midi_from(origin, mapped, Some(profile.id.as_str()));
        if let Some(command) = map.interpret_with(profile, mapped, src_channel) {
            let _ = send(command);
        }
    }
}

pub fn list_ports() -> (Vec<String>, Vec<String>) {
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    if let Ok(input) = MidiInput::new("jambox-list") {
        for port in input.ports() {
            if let Ok(name) = input.port_name(&port) {
                ins.push(name);
            }
        }
    }
    if let Ok(output) = MidiOutput::new("jambox-list") {
        for port in output.ports() {
            if let Ok(name) = output.port_name(&port) {
                outs.push(name);
            }
        }
    }
    (ins, outs)
}

/// Live IN/OUT filters and the ports actually open. The kiosk writes filters
/// over the control socket; the hotplug threads re-open without a restart.
#[derive(Default)]
pub struct MidiIo {
    in_filter: Mutex<String>,
    out_filter: Mutex<String>,
    in_connected: Mutex<String>,
    out_connected: Mutex<String>,
    channel_map: Mutex<[u16; 16]>,
    /// Pad LED note/velocity pairs for the controller's External/Third port.
    led_queue: Mutex<Vec<(u8, u8)>>,
}

impl MidiIo {
    pub fn new(in_filter: String, out_filter: String) -> Self {
        Self {
            in_filter: Mutex::new(in_filter),
            out_filter: Mutex::new(out_filter),
            in_connected: Mutex::new(String::new()),
            out_connected: Mutex::new(String::new()),
            channel_map: Mutex::new([0; 16]),
            led_queue: Mutex::new(Vec::new()),
        }
    }

    pub fn queue_clip_led(&self, command: &Command) {
        match *command {
            Command::LaunchClip { slot, .. } if slot < 16 => self.queue_pad_led(slot, true),
            Command::StopClip { slot, .. } if slot < 16 => self.queue_pad_led(slot, false),
            Command::StopAllClips => {
                for slot in 0..16u8 {
                    self.queue_pad_led(slot, false);
                }
            }
            _ => {}
        }
    }

    fn queue_pad_led(&self, slot: u8, on: bool) {
        let profile = profile_for_port_name(&self.input_connected());
        if profile.feedback_port_hints.is_empty() {
            return;
        }
        let note = profile.note_for_phrase_cell(slot as usize);
        self.led_queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((note, if on { 127 } else { 0 }));
    }

    pub fn set_channel_map(&self, bits: [u16; 16]) {
        *self.channel_map.lock().unwrap_or_else(|p| p.into_inner()) = bits;
    }

    pub fn dest_mask(&self, channel: u8) -> u16 {
        let bits = *self.channel_map.lock().unwrap_or_else(|p| p.into_inner());
        fanout_dest_mask(bits[(channel & 0x0f) as usize], channel)
    }

    fn lock_string(slot: &Mutex<String>) -> String {
        slot.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn store_string(slot: &Mutex<String>, value: String) {
        *slot.lock().unwrap_or_else(|p| p.into_inner()) = value;
    }

    pub fn input_filter(&self) -> String {
        Self::lock_string(&self.in_filter)
    }

    pub fn output_filter(&self) -> String {
        Self::lock_string(&self.out_filter)
    }

    pub fn set_input_filter(&self, filter: String) {
        Self::store_string(&self.in_filter, filter);
    }

    pub fn set_output_filter(&self, filter: String) {
        Self::store_string(&self.out_filter, filter);
    }

    pub fn input_connected(&self) -> String {
        Self::lock_string(&self.in_connected)
    }

    pub fn output_connected(&self) -> String {
        Self::lock_string(&self.out_connected)
    }

    fn set_input_connected_list(&self, names: impl IntoIterator<Item = String>) {
        let mut names: Vec<String> = names.into_iter().collect();
        names.sort();
        Self::store_string(&self.in_connected, names.join(" | "));
    }

    fn set_output_connected(&self, name: String) {
        Self::store_string(&self.out_connected, name);
    }

    pub fn snapshot(&self) -> jambox_protocol::MidiPortsReply {
        let (inputs, outputs) = list_ports();
        jambox_protocol::MidiPortsReply {
            inputs,
            outputs,
            input_filter: self.input_filter(),
            output_filter: self.output_filter(),
            input_connected: self.input_connected(),
            output_connected: self.output_connected(),
        }
    }
}

fn wanted_input_names(filter: &str) -> Vec<String> {
    let (ins, _) = list_ports();
    matching_port_names(ins.iter().map(String::as_str), filter)
}

fn pick_output_name(filter: &str) -> Option<String> {
    let (_, outs) = list_ports();
    pick_thru_output_name(outs.iter().map(String::as_str), filter)
}

/// Watch MIDI inputs and connect when matching ports appear (or return).
///
/// An empty filter opens every class-compliant hardware USB MIDI device.
pub fn spawn_input(
    io: Arc<MidiIo>,
    side: Arc<std::sync::Mutex<MidiInSide>>,
    hub: Arc<crate::ipc::ClientHub>,
    map: Arc<MidiMap>,
    running: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let shared_side = side;
        let mut connections: HashMap<String, MidiInputConnection<()>> = HashMap::new();
        let mut announced_wait = false;
        let mut last_filter = String::from("\0");
        while running.load(Ordering::Relaxed) {
            let filter = io.input_filter();
            if filter != last_filter {
                if !filter.trim().is_empty() {
                    info!(filter = %filter, "midi: watching for input (hotplug)");
                } else {
                    info!("midi: watching every class-compliant USB MIDI input");
                }
                last_filter = filter.clone();
                if !connections.is_empty() {
                    info!("midi: input filter changed; reconnecting");
                    connections.clear();
                    io.set_input_connected_list(Vec::new());
                }
            }
            let wanted = wanted_input_names(&filter);
            connections.retain(|name, _| {
                if wanted.iter().any(|n| n == name) {
                    true
                } else {
                    info!(port = %name, "midi: input gone; waiting for reconnect");
                    false
                }
            });
            for name in &wanted {
                if connections.contains_key(name) {
                    continue;
                }
                announced_wait = false;
                match try_connect(
                    name,
                    Arc::clone(&io),
                    Arc::clone(&shared_side),
                    Arc::clone(&hub),
                    Arc::clone(&map),
                ) {
                    Ok(conn) => {
                        info!(port = %name, "midi: input open");
                        connections.insert(name.clone(), conn);
                    }
                    Err(err) => {
                        warn!(%err, port = %name, "midi: input connect failed");
                    }
                }
            }
            io.set_input_connected_list(connections.keys().cloned());
            if connections.is_empty() && !announced_wait {
                announced_wait = true;
                info!("midi: no class-compliant USB MIDI input yet; will grab any that appear");
            }
            std::thread::sleep(HOTPLUG_POLL);
        }
    });
}

fn try_connect(
    name: &str,
    io: Arc<MidiIo>,
    side: Arc<std::sync::Mutex<MidiInSide>>,
    hub: Arc<crate::ipc::ClientHub>,
    map: Arc<MidiMap>,
) -> Result<MidiInputConnection<()>, MidiError> {
    let mut input = MidiInput::new("jambox-in").map_err(|e| MidiError::Init(e.to_string()))?;
    input.ignore(midir::Ignore::SysexAndTime);
    let ports = input.ports();
    let chosen = ports
        .iter()
        .find(|p| input.port_name(p).ok().as_deref() == Some(name))
        .cloned()
        .ok_or_else(|| MidiError::NoPort(name.to_string()))?;
    let profile = profile_for_port_name(name);
    input
        .connect(
            &chosen,
            "jambox-in",
            move |_stamp, bytes, _| {
                if let Some(event) = MidiEvent::parse(bytes) {
                    let dest_mask = io.dest_mask(event.channel());
                    let mut guard = side.lock().unwrap_or_else(|p| p.into_inner());
                    ingest(event, dest_mask, None, &hub, &map, &profile, |c| {
                        guard.send(c)
                    });
                }
            },
            (),
        )
        .map_err(|e| MidiError::Connect(e.to_string()))
}

fn try_open_output(name: &str) -> Result<MidiOutputConnection, MidiError> {
    let output = MidiOutput::new("jambox-out").map_err(|e| MidiError::Init(e.to_string()))?;
    let ports = output.ports();
    let chosen = ports
        .iter()
        .find(|p| output.port_name(p).ok().as_deref() == Some(name))
        .cloned()
        .ok_or_else(|| MidiError::NoPort(name.to_string()))?;
    output
        .connect(&chosen, "jambox-out")
        .map_err(|e| MidiError::Connect(e.to_string()))
}

/// Drain engine-emitted MIDI to a hardware port until `running` clears.
/// Re-opens when the cable returns or the kiosk changes the OUT filter.
pub fn spawn_output(io: Arc<MidiIo>, mut side: MidiOutSide, running: Arc<AtomicBool>) {
    let _ = std::thread::Builder::new()
        .name("jambox-midi-out".into())
        .spawn(move || {
        let mut connection: Option<MidiOutputConnection> = None;
        let mut current = String::new();
        let mut announced_wait = false;
        let mut last_filter = String::from("\0");
        let mut buf = [0u8; 3];
        let mut next_scan = Instant::now();
        while running.load(Ordering::Relaxed) {
            let filter = io.output_filter();
            if filter != last_filter {
                if !filter.trim().is_empty() {
                    info!(filter = %filter, "midi: watching for output (hotplug)");
                } else {
                    info!("midi: watching for a hardware output (hotplug)");
                }
                last_filter = filter.clone();
                if connection.is_some() {
                    info!(port = %current, "midi: output filter changed; reconnecting");
                    connection = None;
                    current.clear();
                    io.set_output_connected(String::new());
                }
                next_scan = Instant::now();
            }
            if Instant::now() >= next_scan {
                next_scan = Instant::now() + HOTPLUG_POLL;
                let wanted = pick_output_name(&filter);
                let still = wanted.as_ref().map(|n| n == &current).unwrap_or(false)
                    && connection.is_some();
                if connection.is_some() && !still {
                    info!(port = %current, "midi: output gone; waiting for reconnect");
                    connection = None;
                    current.clear();
                    io.set_output_connected(String::new());
                }
                if connection.is_none() {
                    if let Some(name) = wanted {
                        announced_wait = false;
                        match try_open_output(&name) {
                            Ok(conn) => {
                                info!(port = %name, "midi: output open");
                                current = name.clone();
                                io.set_output_connected(name);
                                connection = Some(conn);
                            }
                            Err(err) => {
                                warn!(%err, port = %name, "midi: output connect failed");
                            }
                        }
                    } else if !announced_wait {
                        announced_wait = true;
                        info!("midi: no matching output yet; clip MIDI stays local until one appears");
                    }
                }
            }
            let mut idle = true;
            while let Ok(event) = side.events.pop() {
                idle = false;
                if let Some(conn) = connection.as_mut() {
                    let n = event.encode(&mut buf);
                    let _ = conn.send(&buf[..n]);
                }
            }
            if idle {
                std::thread::sleep(if connection.is_some() {
                    OUT_POLL
                } else {
                    HOTPLUG_POLL
                });
            }
        }
    });
}

/// Note-on/off bytes for RGB / pad LEDs. Never send these to the DIN synth.
pub fn pad_led_bytes(note: u8, velocity: u8) -> [u8; 3] {
    if velocity == 0 {
        [0x80, note, 0]
    } else {
        [0x90, note, velocity]
    }
}

/// Elite-style LED ports are named External / Third on the same USB device.
pub fn pick_feedback_port(outputs: &[String], profile: &ControllerProfile) -> Option<String> {
    if profile.feedback_port_hints.is_empty() {
        return None;
    }
    outputs
        .iter()
        .find(|name| {
            let n = name.to_ascii_lowercase();
            profile.matches_port(name)
                && profile
                    .feedback_port_hints
                    .iter()
                    .any(|hint| n.contains(&hint.to_ascii_lowercase()))
        })
        .cloned()
}

/// Open the controller's LED port and push phrase-pad note-on/off. Skips the
/// DIN thru output so U2MIDI never sees pad lamps.
pub fn spawn_feedback(io: Arc<MidiIo>, running: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut connection: Option<midir::MidiOutputConnection> = None;
        let mut current = String::new();
        while running.load(Ordering::Relaxed) {
            let profile = profile_for_port_name(&io.input_connected());
            let (_, outs) = list_ports();
            let wanted = pick_feedback_port(&outs, &profile);
            let still =
                wanted.as_ref().map(|n| n == &current).unwrap_or(false) && connection.is_some();
            if connection.is_some() && !still {
                connection = None;
                current.clear();
            }
            if connection.is_none() {
                if let Some(name) = wanted {
                    if let Ok(conn) = try_open_output(&name) {
                        info!(port = %name, "midi: pad LED feedback open");
                        current = name;
                        connection = Some(conn);
                    }
                }
            }
            let pending = {
                let mut q = io.led_queue.lock().unwrap_or_else(|p| p.into_inner());
                std::mem::take(&mut *q)
            };
            if let Some(conn) = connection.as_mut() {
                for (note, vel) in pending {
                    let bytes = pad_led_bytes(note, vel);
                    let _ = conn.send(&bytes);
                }
            }
            std::thread::sleep(OUT_POLL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_pad_hold_starts_and_stops_repeat() {
        let map = MidiMap::default();
        let mut slots = [0u8; 16];
        slots[jambox_core::DrumModel::Kick.index()] = 1;
        map.set_drum_repeat(slots, false);
        assert_eq!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 36,
                velocity: 80,
            }),
            Some(Command::StartRepeat {
                owner: 0x1000 | 36,
                channel: 9,
                note: 36,
                velocity: 80,
                division: RepeatDivision::Quarter,
            })
        );
        assert_eq!(
            map.interpret(MidiEvent::NoteOff {
                channel: 9,
                note: 36,
                velocity: 0,
            }),
            Some(Command::StopRepeat { owner: 0x1000 | 36 })
        );
    }

    #[test]
    fn hardware_pad_lock_keeps_repeat_after_release() {
        let map = MidiMap::default();
        let mut slots = [0u8; 16];
        slots[jambox_core::DrumModel::Kick.index()] = 2;
        map.set_drum_repeat(slots, true);
        assert!(matches!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 36,
                velocity: 100,
            }),
            Some(Command::StartRepeat {
                division: RepeatDivision::Eighth,
                ..
            })
        ));
        assert_eq!(
            map.interpret(MidiEvent::NoteOff {
                channel: 9,
                note: 36,
                velocity: 0,
            }),
            None
        );
        assert_eq!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 36,
                velocity: 100,
            }),
            Some(Command::StopRepeat { owner: 0x1000 | 36 })
        );
    }

    #[test]
    fn hardware_pad_lock_two_pads_toggle_independently() {
        let map = MidiMap::default();
        let mut slots = [0u8; 16];
        slots[jambox_core::DrumModel::Kick.index()] = 2;
        slots[jambox_core::DrumModel::Snare.index()] = 2;
        map.set_drum_repeat(slots, true);
        assert!(matches!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 36,
                velocity: 100,
            }),
            Some(Command::StartRepeat { note: 36, .. })
        ));
        assert!(matches!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 45,
                velocity: 100,
            }),
            Some(Command::StartRepeat { note: 45, .. })
        ));
        assert_eq!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 45,
                velocity: 100,
            }),
            Some(Command::StopRepeat { owner: 0x1000 | 45 })
        );
        assert!(matches!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 36,
                velocity: 100,
            }),
            Some(Command::StartRepeat { note: 36, .. })
        ));
        assert_eq!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 36,
                velocity: 100,
            }),
            Some(Command::StopRepeat { owner: 0x1000 | 36 })
        );
    }

    #[test]
    fn ui_latched_repeat_syncs_hw_state_for_midi_retap() {
        let map = MidiMap::default();
        let mut slots = [0u8; 16];
        slots[jambox_core::DrumModel::Snare.index()] = 2;
        map.set_drum_repeat(slots, true);
        map.set_hw_repeat_latched(45, true);
        assert_eq!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 45,
                velocity: 100,
            }),
            Some(Command::StopRepeat { owner: 0x1000 | 45 })
        );
    }

    #[test]
    fn snap_to_scale_walks_unique_degrees() {
        let major = jambox_protocol::default_scale_mask();
        // C major from C4: successive keys climb the scale (no C♯→C duplicate).
        assert_eq!(snap_to_scale(60, 0, major), 60); // C4
        assert_eq!(snap_to_scale(61, 0, major), 62); // → D4
        assert_eq!(snap_to_scale(62, 0, major), 64); // → E4
        assert_eq!(snap_to_scale(66, 0, major), 71); // → B4
        assert_eq!(snap_to_scale(67, 0, major), 72); // → C5
                                                     // A minor pentatonic (A C D E G) rooted at A.
        let pent: u16 = 1 | 1 << 3 | 1 << 5 | 1 << 7 | 1 << 10;
        assert_eq!(snap_to_scale(57, 9, pent), 57); // A3
        assert_eq!(snap_to_scale(58, 9, pent), 60); // → C4
        assert_eq!(snap_to_scale(61, 0, 0), 61);
        assert_eq!(snap_to_scale(61, 0, 0x0fff), 61);
        assert_eq!(snap_to_scale(127, 0, major), 127);
    }

    #[test]
    fn snap_to_scale_adjacent_keys_are_distinct_for_common_scales() {
        let ids = [
            "ionian",
            "dorian",
            "aeolian",
            "major_pent",
            "minor_pent",
            "blues",
            "whole",
            "harmonic",
            "ryukyu",
        ];
        for id in ids {
            let scale = jambox_core::kaoss_scale(jambox_core::kaoss_scale_index_by_id(id) as usize);
            let mask = scale.degrees.iter().fold(0u16, |m, d| m | 1 << (d % 12));
            assert!(
                mask != 0 && mask != 0x0fff,
                "{id} should be a proper subset"
            );
            for root in 0..12u8 {
                let mut prev = None;
                // Stay clear of MIDI 0/127 clamp saturation at the extremes.
                for note in 24u8..=100 {
                    let mapped = snap_to_scale(note, root, mask);
                    if let Some(p) = prev {
                        assert_ne!(
                            mapped, p,
                            "{id} root={root}: MIDI {note} mapped to {mapped}, same as previous key"
                        );
                    }
                    prev = Some(mapped);
                }
            }
        }
    }

    #[test]
    fn scale_lock_snaps_keys_and_releases_what_it_played() {
        let map = MidiMap::default();
        let profile = ControllerProfile::mpk_mini();
        let on = MidiEvent::NoteOn {
            channel: 0,
            note: 61,
            velocity: 100,
        };
        assert_eq!(map.apply_scale(on, &profile), on, "off by default");
        map.set_scale_lock(true, 0, jambox_protocol::default_scale_mask());
        assert_eq!(
            map.apply_scale(on, &profile),
            MidiEvent::NoteOn {
                channel: 0,
                note: 62,
                velocity: 100
            },
            "C♯4 walks to D4 in C major"
        );
        // Scale changes while held: the note-off still releases what note-on played.
        map.set_scale_lock(true, 1, 0b1);
        assert_eq!(
            map.apply_scale(
                MidiEvent::NoteOff {
                    channel: 0,
                    note: 61,
                    velocity: 0
                },
                &profile
            ),
            MidiEvent::NoteOff {
                channel: 0,
                note: 62,
                velocity: 0
            }
        );
        // Drum pads pass through.
        let pad = MidiEvent::NoteOn {
            channel: 9,
            note: 37,
            velocity: 100,
        };
        assert_eq!(map.apply_scale(pad, &profile), pad);
    }

    #[test]
    fn full_vel_rewrites_every_note_on_including_drums() {
        let map = MidiMap::default();
        assert!(map.full_vel());
        assert_eq!(
            map.apply_velocity(MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 40,
            }),
            MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 127,
            }
        );
        assert_eq!(
            map.apply_velocity(MidiEvent::NoteOn {
                channel: 9,
                note: 36,
                velocity: 40,
            }),
            MidiEvent::NoteOn {
                channel: 9,
                note: 36,
                velocity: 127,
            }
        );
        assert_eq!(
            map.apply_velocity(MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 0,
            }),
            MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 0,
            },
            "note-on velocity 0 is a note-off; leave it"
        );
        map.set_full_vel(false);
        assert_eq!(
            map.apply_velocity(MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 40,
            }),
            MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 40,
            }
        );
    }

    #[test]
    fn stick_and_aftertouch_follow_live_assigns() {
        let map = MidiMap::default();
        assert_eq!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MOD,
                value: 127,
            }),
            Some(Command::SetSynth {
                param: SynthParam::VibratoMod,
                value: 1.0
            })
        );
        map.set_assigns(None, Some("tone"), Some("vibrato"));
        assert_eq!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MOD,
                value: 64,
            }),
            Some(Command::SetSynth {
                param: SynthParam::Tone,
                value: 64.0 / 127.0
            })
        );
        assert_eq!(
            map.interpret(MidiEvent::PolyPressure {
                channel: 9,
                note: 36,
                pressure: 90,
            }),
            Some(Command::SetSynth {
                param: SynthParam::VibratoMod,
                value: 90.0 / 127.0
            })
        );
        map.set_assigns(Some("off"), None, Some("off"));
        assert!(map
            .interpret(MidiEvent::PitchBend {
                channel: 0,
                value: 16_383,
            })
            .is_none());
        assert!(map
            .interpret(MidiEvent::PolyPressure {
                channel: 9,
                note: 36,
                pressure: 40,
            })
            .is_none());
    }

    #[test]
    fn drum_poly_pressure_opens_that_pad_tone() {
        let map = MidiMap::default();
        assert_eq!(
            map.interpret(MidiEvent::PolyPressure {
                channel: 9,
                note: 36,
                pressure: 64,
            }),
            Some(Command::SetDrumAftertouch {
                model: 0,
                amount: 64.0 / 127.0,
            })
        );
        assert_eq!(
            map.interpret(MidiEvent::PolyPressure {
                channel: 9,
                note: 37,
                pressure: 127,
            }),
            Some(Command::SetDrumAftertouch {
                model: drum_model_for_note(37).index() as u8,
                amount: 1.0,
            })
        );
        assert!(
            map.interpret(MidiEvent::PolyPressure {
                channel: 0,
                note: 60,
                pressure: 90,
            })
            .is_none(),
            "key aftertouch must not kit-wide stomp drum macros without a pad"
        );
    }

    #[test]
    fn punch_page_keeps_pad_aftertouch_for_fx() {
        let map = MidiMap::default();
        map.set_punch();
        assert!(map
            .interpret(MidiEvent::PolyPressure {
                channel: 9,
                note: 36,
                pressure: 80,
            })
            .is_none());
        map.set_drums();
        assert_eq!(
            map.interpret(MidiEvent::PolyPressure {
                channel: 9,
                note: 36,
                pressure: 80,
            }),
            Some(Command::SetDrumAftertouch {
                model: 0,
                amount: 80.0 / 127.0,
            })
        );
    }

    #[test]
    fn note_on_becomes_a_note_command() {
        let map = MidiMap::default();
        let command = map.interpret(MidiEvent::NoteOn {
            channel: 9,
            note: 36,
            velocity: 110,
        });
        assert_eq!(
            command,
            Some(Command::NoteOn {
                channel: 9,
                note: 36,
                velocity: 110
            })
        );
    }

    #[test]
    fn note_off_becomes_a_release() {
        let map = MidiMap::default();
        let command = map.interpret(MidiEvent::NoteOff {
            channel: 0,
            note: 60,
            velocity: 0,
        });
        assert_eq!(
            command,
            Some(Command::NoteOff {
                channel: 0,
                note: 60
            })
        );
    }

    #[test]
    fn mod_wheel_scales_vibrato_instead_of_replacing_depth() {
        let map = MidiMap::default();
        let command = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MOD,
                value: 127,
            })
            .unwrap();
        assert_eq!(
            command,
            Command::SetSynth {
                param: SynthParam::VibratoMod,
                value: 1.0
            }
        );
    }

    #[test]
    fn key_knobs_follow_the_live_controller_profile() {
        assert_eq!(
            ControllerProfile::mpk_mini().knob_ccs,
            [
                CC_MORPH,
                CC_TONE,
                CC_ATTACK,
                CC_RELEASE,
                CC_VIB_DEPTH,
                CC_VIB_RATE,
                76,
                CC_LEVEL
            ]
        );
        let map = MidiMap::default();
        let mut smk = ControllerProfile::smk_37_elite();
        smk.knob_encoding = KnobEncoding::Absolute;
        assert_eq!(
            map.interpret_with(
                &smk,
                MidiEvent::ControlChange {
                    channel: 0,
                    controller: 48,
                    value: 127,
                },
                0
            ),
            Some(Command::SetSynth {
                param: SynthParam::Morph,
                value: 1.0
            })
        );
        assert!(
            map.interpret_with(
                &smk,
                MidiEvent::ControlChange {
                    channel: 0,
                    controller: CC_MORPH,
                    value: 127,
                },
                0
            )
            .is_none(),
            "MPK CC 70 must not steal a controller that remapped knobs"
        );
        assert_eq!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MORPH,
                value: 127,
            }),
            Some(Command::SetSynth {
                param: SynthParam::Morph,
                value: 1.0
            }),
            "MPK path stays on factory CCs"
        );
    }

    #[test]
    fn smk_transport_notes_do_not_wrap_into_the_kit() {
        let map = MidiMap::default();
        let smk = ControllerProfile::smk_37_elite();
        assert!(map
            .interpret_with(
                &smk,
                MidiEvent::NoteOn {
                    channel: 9,
                    note: 91,
                    velocity: 100,
                },
                9
            )
            .is_none());
        assert_eq!(
            map.interpret_with(
                &smk,
                MidiEvent::NoteOn {
                    channel: 9,
                    note: 36,
                    velocity: 100,
                },
                9
            ),
            Some(Command::NoteOn {
                channel: 9,
                note: 36,
                velocity: 100
            })
        );
        assert_eq!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 91,
                velocity: 100,
            }),
            Some(Command::NoteOn {
                channel: 9,
                note: 91,
                velocity: 100
            }),
            "MPK has no transport pad bank"
        );
    }

    #[test]
    fn pad_led_feedback_targets_external_not_din() {
        let smk = ControllerProfile::smk_37_elite();
        let outs = vec![
            "U2MIDI PRO:U2MIDI PRO MIDI 1 20:0".into(),
            "SMK-37 Elite-External:SMK-37 Elite-External MIDI 1 24:0".into(),
            "SMK-37 Elite-Master:SMK-37 Elite-Master MIDI 1 20:0".into(),
        ];
        assert_eq!(
            pick_feedback_port(&outs, &smk).as_deref(),
            Some("SMK-37 Elite-External:SMK-37 Elite-External MIDI 1 24:0")
        );
        assert!(pick_feedback_port(&outs, &ControllerProfile::mpk_mini()).is_none());
        assert_eq!(pad_led_bytes(36, 127), [0x90, 36, 127]);
        assert_eq!(pad_led_bytes(36, 0), [0x80, 36, 0]);
    }

    #[test]
    fn sustain_cc64_is_not_a_fader_on_the_elite() {
        let map = MidiMap::default();
        assert_eq!(
            map.interpret_with(
                &ControllerProfile::smk_37_elite(),
                MidiEvent::ControlChange {
                    channel: 0,
                    controller: 64,
                    value: 127,
                },
                0
            ),
            Some(Command::SetSynth {
                param: SynthParam::Sustain,
                value: 1.0
            })
        );
        assert_eq!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: 64,
                value: 0,
            }),
            Some(Command::SetSynth {
                param: SynthParam::Sustain,
                value: 0.0
            })
        );
    }

    #[test]
    fn smk_encoder_bank2_drives_bus_fx() {
        let map = MidiMap::default();
        let mut smk = ControllerProfile::smk_37_elite();
        smk.knob_encoding = KnobEncoding::Absolute;
        match map.interpret_with(
            &smk,
            MidiEvent::ControlChange {
                channel: 0,
                controller: 56,
                value: 127,
            },
        ) {
            Some(Command::SetFx {
                target: FxTarget::Bus,
                param: FxParam::Drive,
                value,
            }) => assert!((value - 1.0).abs() < 1e-5),
            other => panic!("{other:?}"),
        }
        assert!(
            map.interpret_with(
                &ControllerProfile::mpk_mini(),
                MidiEvent::ControlChange {
                    channel: 0,
                    controller: 56,
                    value: 127,
                },
                0
            )
            .is_none(),
            "MPK has no encoder bank 2"
        );
    }

    #[test]
    fn smk_faders_drive_the_mix_buses() {
        let map = MidiMap::default();
        let smk = ControllerProfile::smk_37_elite();
        assert_eq!(
            map.interpret_with(
                &smk,
                MidiEvent::ControlChange {
                    channel: 0,
                    controller: 20,
                    value: 127,
                },
                0
            ),
            Some(Command::SetSynth {
                param: SynthParam::Level,
                value: 1.0
            })
        );
        assert_eq!(
            map.interpret_with(
                &smk,
                MidiEvent::ControlChange {
                    channel: 0,
                    controller: 21,
                    value: 64,
                },
                0
            ),
            Some(Command::SetSynth {
                param: SynthParam::DrumLevel,
                value: 64.0 / 127.0
            })
        );
        match map.interpret_with(
            &smk,
            MidiEvent::ControlChange {
                channel: 0,
                controller: 23,
                value: 64,
            },
        ) {
            Some(Command::SetClipGain { slot, value }) => {
                assert_eq!(slot, SEQ_CLIP_SLOT);
                assert!((value - (64.0 / 127.0) * 2.0).abs() < 1e-5);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn relative_encoders_nudge_from_the_last_value() {
        let map = MidiMap::default();
        let smk = ControllerProfile::smk_37_elite();
        assert_eq!(smk.knob_encoding, KnobEncoding::RelativeSigned);
        let first = map
            .interpret_with(
                &smk,
                MidiEvent::ControlChange {
                    channel: 0,
                    controller: 48,
                    value: 1,
                },
            )
            .unwrap();
        match first {
            Command::SetSynth {
                param: SynthParam::Morph,
                value,
            } => assert!((value - 1.0 / 64.0).abs() < 1e-5),
            other => panic!("{other:?}"),
        }
        let back = map
            .interpret_with(
                &smk,
                MidiEvent::ControlChange {
                    channel: 0,
                    controller: 48,
                    value: 127,
                },
            )
            .unwrap();
        match back {
            Command::SetSynth {
                param: SynthParam::Morph,
                value,
            } => assert!(value.abs() < 1e-5, "127 is −1 tick, got {value}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn key_knobs_move_synth_params() {
        let map = MidiMap::default();
        let command = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_TONE,
                value: 127,
            })
            .unwrap();
        assert_eq!(
            command,
            Command::SetSynth {
                param: SynthParam::Tone,
                value: 1.0
            }
        );
        let level = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_ATTACK,
                value: 127,
            })
            .unwrap();
        assert_eq!(
            level,
            Command::SetSynth {
                param: SynthParam::Level,
                value: 1.0
            }
        );
        let drift = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_VIB_RATE,
                value: 64,
            })
            .unwrap();
        match drift {
            Command::SetSynth {
                param: SynthParam::ReleaseDrift,
                value,
            } => assert!((value - 64.0 / 127.0).abs() < 0.001),
            other => panic!("{other:?}"),
        }
        let flange = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: 76,
                value: 127,
            })
            .unwrap();
        assert_eq!(
            flange,
            Command::SetFx {
                target: FxTarget::Voice(0),
                param: FxParam::FlangerMix,
                value: 1.0
            }
        );
        let rate = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_LEVEL,
                value: 127,
            })
            .unwrap();
        assert_eq!(
            rate,
            Command::SetFx {
                target: FxTarget::Voice(0),
                param: FxParam::FlangerRate,
                value: 1.0
            }
        );
    }

    #[test]
    fn drum_mode_rebinds_the_same_ccs() {
        let map = MidiMap::default();
        map.set_drums();
        assert!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MORPH,
                value: 64,
            })
            .is_none(),
            "drums-mode knobs stay quiet until a pad names the target voice"
        );
        assert_eq!(
            map.interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 45,
                velocity: 100,
            }),
            Some(Command::NoteOn {
                channel: 9,
                note: 45,
                velocity: 100
            })
        );
        let command = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MORPH,
                value: 64,
            })
            .unwrap();
        match command {
            Command::SetDrumMacro {
                model,
                param: SynthParam::DrumPitch,
                value,
            } => {
                assert_eq!(model, drum_model_for_note(45).index() as u8);
                assert!((value - 64.0 / 127.0).abs() < 0.001);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn drum_level_expression_hits_only_the_last_pad() {
        let map = MidiMap::default();
        map.set_assigns(None, Some("drum_level"), Some("drum_level"));
        assert!(
            map.interpret(MidiEvent::ChannelPressure {
                channel: 9,
                pressure: 100,
            })
            .is_none(),
            "channel pressure with no prior pad must not raise the whole kit"
        );
        assert!(map
            .interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 45,
                velocity: 110,
            })
            .is_some());
        assert_eq!(
            map.interpret(MidiEvent::ChannelPressure {
                channel: 9,
                pressure: 100,
            }),
            Some(Command::SetDrumMacro {
                model: drum_model_for_note(45).index() as u8,
                param: SynthParam::DrumAmp,
                value: 100.0 / 127.0,
            })
        );
        assert!(map
            .interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 36,
                velocity: 40,
            })
            .is_some());
        assert_eq!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MOD,
                value: 20,
            }),
            Some(Command::SetDrumMacro {
                model: 0,
                param: SynthParam::DrumAmp,
                value: 20.0 / 127.0,
            }),
            "stick Y drum-level follows the last pad (kick), not the whole kit"
        );
    }

    #[test]
    fn fm_mode_knobs_are_owned_by_the_kiosk() {
        let map = MidiMap::default();
        map.set_fm();
        assert!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MORPH,
                value: 127,
            })
            .is_none(),
            "FM knobs update kiosk sliders; DSP follows kiosk synth msgs"
        );
        map.apply_knob_map("fm", None, 0);
        assert!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_TONE,
                value: 0,
            })
            .is_none()
        );
    }

    #[test]
    fn punch_mode_eats_mpk_pads_and_knobs() {
        let map = MidiMap::default();
        map.set_punch();
        let profile = ControllerProfile::mpk_mini();
        assert!(map
            .interpret_with(
                &profile,
                MidiEvent::NoteOn {
                    channel: midi_core::WIRE_PAD_CHANNEL,
                    note: 40,
                    velocity: 100,
                },
                midi_core::WIRE_PAD_CHANNEL
            )
            .is_none());
        assert!(map
            .interpret_with(
                &profile,
                MidiEvent::ControlChange {
                    channel: 0,
                    controller: 70,
                    value: 64,
                },
                0
            )
            .is_none());
        assert!(map
            .interpret_with(
                &profile,
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 60,
                    velocity: 100,
                },
                0
            )
            .is_some());
    }

    #[test]
    fn punch_mode_only_steals_wire_ch10_pads() {
        let map = MidiMap::default();
        map.set_punch();
        assert!(map.pads_to_fx());
        let mpk = ControllerProfile::mpk_mini();
        for note in 36u8..=43 {
            assert!(!mpk.is_punch_pad(0, note));
            assert!(mpk.is_fx_pad_hit(9, note));
            assert_eq!(
                map.interpret_with(
                    &mpk,
                    MidiEvent::NoteOn {
                        channel: 0,
                        note,
                        velocity: 100,
                    },
                    0
                ),
                Some(Command::NoteOn {
                    channel: 0,
                    note,
                    velocity: 100
                }),
                "keys ch1 pad-number {note} must play synth"
            );
            assert!(
                map.interpret_with(
                    &mpk,
                    MidiEvent::NoteOn {
                        channel: 9,
                        note,
                        velocity: 100,
                    },
                    9
                )
                .is_none(),
                "wire ch10 pad {note} arms FX, not the kit"
            );
            assert_eq!(
                map.interpret_with(
                    &mpk,
                    MidiEvent::NoteOn {
                        channel: 1,
                        note,
                        velocity: 100,
                    },
                    1
                ),
                Some(Command::NoteOn {
                    channel: 1,
                    note,
                    velocity: 100
                }),
                "other channels keep pad-number pitches"
            );
        }
        // SMK (and factory ch10 maps): pads on drums channel; keys ch1 keep 36–43.
        let smk = ControllerProfile::smk_37_elite();
        assert_eq!(smk.pad_channel_index(), 9);
        assert_eq!(
            map.interpret_with(
                &smk,
                MidiEvent::NoteOn {
                    channel: 0,
                    note: 40,
                    velocity: 100,
                },
                0
            ),
            Some(Command::NoteOn {
                channel: 0,
                note: 40,
                velocity: 100
            }),
            "keybed must keep pad-number pitches when pads live on ch10"
        );
        assert!(map
            .interpret_with(
                &smk,
                MidiEvent::NoteOn {
                    channel: 9,
                    note: 40,
                    velocity: 100,
                },
                9
            )
            .is_none());
    }

    #[test]
    fn midi_ch1_never_reaches_drum_engine_even_when_pad_fx_off() {
        let map = MidiMap::default();
        map.set_punch();
        map.set_pads_to_fx(false);
        let mpk = ControllerProfile::mpk_mini();
        for note in [40u8, 36, 60, 45] {
            assert_eq!(
                map.interpret_with(
                    &mpk,
                    MidiEvent::NoteOn {
                        channel: 0,
                        note,
                        velocity: 100,
                    },
                    0,
                ),
                Some(Command::NoteOn {
                    channel: 0,
                    note,
                    velocity: 100
                }),
                "wire ch1 note {note} must stay on synth channel 0"
            );
            assert_eq!(
                map.interpret_with(
                    &mpk,
                    MidiEvent::NoteOff {
                        channel: 0,
                        note,
                        velocity: 0,
                    },
                    0,
                ),
                Some(Command::NoteOff { channel: 0, note }),
            );
        }
    }

    #[test]
    fn midi_ch10_plays_drums_keys_mode_and_after_pad_fx_off() {
        let map = MidiMap::default();
        let mpk = ControllerProfile::mpk_mini();
        for pads_off in [false, true] {
            map.set_pads_to_fx(pads_off);
            map.set_keys();
            assert_eq!(
                map.interpret_with(
                    &mpk,
                    MidiEvent::NoteOn {
                        channel: DRUM_CHANNEL,
                        note: 36,
                        velocity: 110,
                    },
                    DRUM_CHANNEL,
                ),
                Some(Command::NoteOn {
                    channel: DRUM_CHANNEL,
                    note: 36,
                    velocity: 110
                }),
                "pads_to_fx={pads_off}"
            );
        }
    }

    #[test]
    fn map_fanout_ch1_to_ch10_still_plays_synth_not_drums() {
        let map = MidiMap::default();
        let mpk = ControllerProfile::mpk_mini();
        let event = MidiEvent::NoteOn {
            channel: DRUM_CHANNEL,
            note: 40,
            velocity: 100,
        };
        assert_eq!(
            map.interpret_with(&mpk, event, 0),
            Some(Command::NoteOn {
                channel: 0,
                note: 40,
                velocity: 100
            }),
            "MAP onto ch10 must not arm the kit when wire source is ch1"
        );
    }

    #[test]
    fn map_fanout_ch10_to_ch1_still_plays_drums() {
        let map = MidiMap::default();
        let mpk = ControllerProfile::mpk_mini();
        let event = MidiEvent::NoteOn {
            channel: 0,
            note: 36,
            velocity: 100,
        };
        assert_eq!(
            map.interpret_with(&mpk, event, DRUM_CHANNEL),
            Some(Command::NoteOn {
                channel: DRUM_CHANNEL,
                note: 36,
                velocity: 100
            }),
        );
    }

    #[test]
    fn skips_through_and_engine_loopback_names() {
        assert!(is_virtual_port_name(
            "Midi Through:Midi Through Port-0 14:0"
        ));
        assert!(is_virtual_port_name("jambox-out:jambox-out 129:0"));
        assert!(!is_virtual_port_name("MPK mini 3"));
        assert!(!is_virtual_port_name("U2MIDI PRO:U2MIDI PRO MIDI 1 20:0"));
    }
}
