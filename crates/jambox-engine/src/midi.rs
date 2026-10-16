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
    profile_for_port_name, ControllerProfile, KnobEncoding, MidiEvent,
};
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use tracing::{info, warn};

use crate::bus::{MidiInSide, MidiOutSide};
use jambox_core::{
    drum_model_for_note, Command, FxParam, FxTarget, RepeatDivision, SynthParam, DRUM_CHANNEL,
    DRUM_MODEL_COUNT, SEQ_CLIP_SLOT, SEQ_DRUM_MIX_SLOT, SEQ_KAOSS_MIX_SLOT,
};

/// Repeat-lane owners for hardware pads (`0x1000 | note`).
const HW_REPEAT_OWNER: u32 = 0x1000;

/// How often the output thread checks the ring once a port is open.
/// A 0.5 ms poll re-listed every ALSA MIDI port each wake and pegged a core.
const OUT_POLL: Duration = Duration::from_millis(2);
/// How often we look for a newly plugged controller.
const HOTPLUG_POLL: Duration = Duration::from_millis(400);

/// MPK mini factory knobs (Prog Select → Pad 1 / MPC program).
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
    /// FX page: Bank A pad notes are punch toggles instead of kit hits.
    pads_to_fx: AtomicBool,
    /// Per kit-voice note-repeat slot (0 = off). Hardware pads read this.
    repeat_div: [AtomicU8; DRUM_MODEL_COUNT],
    repeat_latch: AtomicBool,
    hw_latched: [AtomicBool; DRUM_MODEL_COUNT],
    hw_note: [AtomicU8; DRUM_MODEL_COUNT],
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
            pads_to_fx: AtomicBool::new(true),
            repeat_div: std::array::from_fn(|_| AtomicU8::new(0)),
            repeat_latch: AtomicBool::new(false),
            hw_latched: std::array::from_fn(|_| AtomicBool::new(false)),
            hw_note: std::array::from_fn(|_| AtomicU8::new(0)),
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

    pub fn set_pads_to_fx(&self, armed: bool) {
        self.pads_to_fx.store(armed, Ordering::Relaxed);
    }

    fn pads_to_fx(&self) -> bool {
        self.pads_to_fx.load(Ordering::Relaxed)
    }

    pub fn full_vel(&self) -> bool {
        self.full_vel.load(Ordering::Relaxed)
    }

    /// Rewrite keyboard note-ons to 127 when FULL VEL is on. Drums / note-offs stay.
    pub fn set_drum_repeat(&self, slots: [u8; 16], latch: bool) -> [u32; DRUM_MODEL_COUNT] {
        let was_latch = self.repeat_latch.swap(latch, Ordering::Relaxed);
        let mut stops = [0u32; DRUM_MODEL_COUNT];
        for i in 0..DRUM_MODEL_COUNT {
            let slot = slots.get(i).copied().unwrap_or(0);
            self.repeat_div[i].store(slot, Ordering::Relaxed);
            let drop = slot == 0 || (was_latch && !latch);
            if drop && self.hw_latched[i].swap(false, Ordering::Relaxed) {
                let note = self.hw_note[i].load(Ordering::Relaxed);
                stops[i] = HW_REPEAT_OWNER | u32::from(note);
            }
        }
        stops
    }

    fn hw_repeat_owner(note: u8) -> u32 {
        HW_REPEAT_OWNER | u32::from(note)
    }

    fn interpret_drum_repeat(&self, note: u8, velocity: u8, on: bool) -> Option<Command> {
        let division = self.repeat_division_for_note(note)?;
        let model = drum_model_for_note(note).index();
        let latch = self.repeat_latch.load(Ordering::Relaxed);
        if on {
            if latch && self.hw_latched[model].swap(false, Ordering::Relaxed) {
                let prev = self.hw_note[model].load(Ordering::Relaxed);
                return Some(Command::StopRepeat {
                    owner: Self::hw_repeat_owner(prev),
                });
            }
            self.hw_note[model].store(note, Ordering::Relaxed);
            self.hw_latched[model].store(latch, Ordering::Relaxed);
            Some(Command::StartRepeat {
                owner: Self::hw_repeat_owner(note),
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
                owner: Self::hw_repeat_owner(note),
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
            5 => Some(RepeatDivision::QuarterTriplet),
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
        self.interpret_with(&ControllerProfile::mpk_mini(), event)
    }

    /// One MIDI event through a live controller map. Unknown devices use
    /// [`ControllerProfile::generic`] (MPK-compatible knobs/pads).
    pub fn interpret_with(&self, profile: &ControllerProfile, event: MidiEvent) -> Option<Command> {
        match event {
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } => {
                if self.mode.load(Ordering::Relaxed) == MODE_PUNCH
                    && self.pads_to_fx()
                    && profile.punch_index_for_pad(note).is_some()
                {
                    return None;
                }
                if channel == DRUM_CHANNEL {
                    if let Some(command) = self.interpret_drum_repeat(note, velocity, true) {
                        return Some(command);
                    }
                }
                Some(Command::NoteOn {
                    channel,
                    note,
                    velocity,
                })
            }
            MidiEvent::NoteOff { channel, note, .. } => {
                if self.mode.load(Ordering::Relaxed) == MODE_PUNCH
                    && self.pads_to_fx()
                    && profile.punch_index_for_pad(note).is_some()
                {
                    return None;
                }
                if channel == DRUM_CHANNEL && self.repeat_division_for_note(note).is_some() {
                    return self.interpret_drum_repeat(note, 0, false);
                }
                Some(Command::NoteOff { channel, note })
            }
            MidiEvent::PitchBend { value, .. } => {
                // ±2 semitones, same as Python `_bend_range`.
                let semis = ((value as f32) - 8192.0) / 8192.0 * 2.0;
                Some(Command::SetSynth {
                    param: SynthParam::PitchBend,
                    value: semis,
                })
            }
            MidiEvent::ControlChange {
                channel,
                controller,
                value,
            } => self.interpret_cc(profile, channel, controller, value),
            _ => None,
        }
    }

    fn cc_unit(&self, slot: usize, encoding: KnobEncoding, value: u8) -> f32 {
        match encoding {
            KnobEncoding::Absolute => {
                let unit = (value as f32 / 127.0).clamp(0.0, 1.0);
                if slot < self.cc_last.len() {
                    self.cc_last[slot]
                        .store((unit * 10_000.0).round() as u16, Ordering::Relaxed);
                }
                unit
            }
            KnobEncoding::RelativeSigned => {
                let delta = relative_cc_delta(value);
                let last = if slot < self.cc_last.len() {
                    self.cc_last[slot].load(Ordering::Relaxed) as f32 / 10_000.0
                } else {
                    0.0
                };
                let unit = (last + delta as f32 / 64.0).clamp(0.0, 1.0);
                if slot < self.cc_last.len() {
                    self.cc_last[slot]
                        .store((unit * 10_000.0).round() as u16, Ordering::Relaxed);
                }
                unit
            }
        }
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
            // Wheel scales existing depth; it must not overwrite Knob 5.
            return Some(Command::SetSynth {
                param: SynthParam::VibratoMod,
                value: unit,
            });
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
            return Some(Command::SetSynth { param, value: unit });
        }
        if mode == MODE_FM {
            let param = match knob {
                0 => SynthParam::FmBright,
                1 => SynthParam::FmClang,
                2 => SynthParam::FmHit,
                3 => SynthParam::FmTail,
                7 => {
                    return Some(Command::SetSynth {
                        param: SynthParam::Level,
                        value: unit.powf(1.15),
                    });
                }
                _ => return None,
            };
            return Some(Command::SetSynth { param, value: unit });
        }
        let param = match knob {
            0 => SynthParam::Morph,
            1 => SynthParam::Tone,
            2 => SynthParam::Attack,
            3 => SynthParam::Release,
            4 => SynthParam::VibratoDepth,
            5 => SynthParam::VibratoRate,
            7 => {
                return Some(Command::SetSynth {
                    param: SynthParam::Level,
                    value: unit.powf(1.15),
                });
            }
            _ => return None,
        };
        Some(Command::SetSynth { param, value: unit })
    }
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

fn relative_cc_delta(value: u8) -> i8 {
    if value == 0 || value == 64 {
        0
    } else if value < 64 {
        value as i8
    } else {
        (value as i16 - 128) as i8
    }
}

/// Push a MIDI event into DSP and to every UI client.
///
/// `dest_mask` is the live channel-map bitmask (`bit j` = emit on channel `j`).
pub fn ingest(
    event: MidiEvent,
    dest_mask: u16,
    hub: &crate::ipc::ClientHub,
    map: &MidiMap,
    profile: &ControllerProfile,
    mut send: impl FnMut(Command) -> bool,
) {
    let event = map.apply_velocity(event);
    let mask = fanout_dest_mask(dest_mask, event.channel());
    for dest in 0..16u8 {
        if mask & (1 << dest) == 0 {
            continue;
        }
        let mapped = event.with_channel(dest);
        hub.broadcast_midi(mapped, Some(profile.id.as_str()));
        if let Some(command) = map.interpret_with(profile, mapped) {
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
                    ingest(event, dest_mask, &hub, &map, &profile, |c| guard.send(c));
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
    outputs.iter().find(|name| {
        let n = name.to_ascii_lowercase();
        profile.matches_port(name)
            && profile
                .feedback_port_hints
                .iter()
                .any(|hint| n.contains(&hint.to_ascii_lowercase()))
    }).cloned()
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
            let still = wanted.as_ref().map(|n| n == &current).unwrap_or(false) && connection.is_some();
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
                }
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
                }
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
                }
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
                }
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
                }
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
                }
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
        assert_eq!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_VIB_RATE,
                value: 0,
            }),
            Some(Command::SetSynth {
                param: SynthParam::VibratoRate,
                value: 0.0
            })
        );
        assert!(matches!(
            map.interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_LEVEL,
                value: 127,
            }),
            Some(Command::SetSynth {
                param: SynthParam::Level,
                ..
            })
        ));
    }

    #[test]
    fn drum_mode_rebinds_the_same_ccs() {
        let map = MidiMap::default();
        map.set_drums();
        let command = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MORPH,
                value: 64,
            })
            .unwrap();
        match command {
            Command::SetSynth {
                param: SynthParam::DrumPitch,
                value,
            } => assert!((value - 64.0 / 127.0).abs() < 0.001),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn fm_mode_rebinds_knobs_to_the_playground() {
        let map = MidiMap::default();
        map.set_fm();
        let command = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_MORPH,
                value: 127,
            })
            .unwrap();
        assert_eq!(
            command,
            Command::SetSynth {
                param: SynthParam::FmBright,
                value: 1.0
            }
        );
        map.apply_knob_map("fm", None, 0);
        let clang = map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: CC_TONE,
                value: 0,
            })
            .unwrap();
        assert_eq!(
            clang,
            Command::SetSynth {
                param: SynthParam::FmClang,
                value: 0.0
            }
        );
    }

    #[test]
    fn punch_mode_eats_mpk_pads_and_knobs() {
        let map = MidiMap::default();
        map.set_punch();
        assert!(map
            .interpret(MidiEvent::NoteOn {
                channel: 9,
                note: 40,
                velocity: 100,
            })
            .is_none());
        assert!(map
            .interpret(MidiEvent::ControlChange {
                channel: 0,
                controller: 70,
                value: 64,
            })
            .is_none());
        assert!(map
            .interpret(MidiEvent::NoteOn {
                channel: 0,
                note: 60,
                velocity: 100,
            })
            .is_some());
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
