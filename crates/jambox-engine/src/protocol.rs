//! Line-delimited JSON control protocol.
//!
//! The UI speaks this over a socket. Parsing happens on the control thread; the
//! audio thread only ever sees the decoded [`Command`] (plain `Copy` data) or a
//! preallocated clip handed over as a `Box`.

use jambox_core::{
    Clip, ClipEvent, ClipEventKind, Command, FxParam, FxTarget, LaunchMode, Quantize,
    RepeatDivision, SynthParam,
};
use jambox_protocol::{
    HelloReply, RepeatDivision as WireRepeatDivision, RepeatPhase, TouchPhase,
};
use serde::{Deserialize, Serialize};

/// One request from the UI.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Hello {
        protocol: u16,
        client: String,
        #[serde(default)]
        realtime_owner: bool,
    },
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    AllNotesOff,
    Panic,
    Synth {
        param: String,
        value: f32,
    },
    Fx {
        target: FxTargetSpec,
        param: String,
        value: f32,
    },
    MorphPair {
        a: u16,
        b: u16,
    },
    Tempo {
        bpm: f32,
    },
    BeatsPerBar {
        beats: u8,
    },
    ClipLoad {
        slot: u8,
        #[serde(default)]
        length_ticks: u32,
        #[serde(default)]
        mode: Option<String>,
        events: Vec<WireClipEvent>,
    },
    ClipClear {
        slot: u8,
    },
    ClipMode {
        slot: u8,
        mode: String,
    },
    ClipLaunch {
        slot: u8,
        #[serde(default)]
        quantize: Option<String>,
    },
    ClipStop {
        slot: u8,
        #[serde(default)]
        quantize: Option<String>,
    },
    StopAllClips,
    Status,
    /// Inject MIDI as if it arrived on the hardware input (touch/KAOSS, tests).
    Midi {
        kind: String,
        #[serde(default)]
        channel: u8,
        #[serde(default)]
        note: Option<u8>,
        #[serde(default)]
        velocity: Option<u8>,
        #[serde(default)]
        control: Option<u8>,
        #[serde(default)]
        value: Option<u16>,
    },
    /// Tell ingest which bank hardware knobs currently address.
    KnobMap {
        mode: String,
        #[serde(default)]
        fx_kind: Option<String>,
        #[serde(default)]
        fx_index: u16,
    },
    Touch {
        gesture: u32,
        phase: TouchPhase,
        x: f32,
        y: f32,
        #[serde(default)]
        channel: u8,
        #[serde(default = "default_velocity")]
        velocity: u8,
    },
    Repeat {
        gesture: u32,
        phase: RepeatPhase,
        note: u8,
        #[serde(default = "default_drum_channel")]
        channel: u8,
        #[serde(default = "default_velocity")]
        velocity: u8,
        #[serde(default = "default_repeat_division")]
        division: WireRepeatDivision,
    },
    KaossScale {
        #[serde(default)]
        scale_index: u8,
        #[serde(default)]
        key: u8,
        #[serde(default = "default_kaoss_root")]
        root_midi: u8,
        #[serde(default = "default_kaoss_octaves")]
        octaves: u8,
    },
    /// Direct MIDI to the engine's USB/DIN out path (not inject-in).
    MidiEmit {
        kind: String,
        channel: u8,
        #[serde(default)]
        note: Option<u8>,
        #[serde(default)]
        velocity: Option<u8>,
        #[serde(default)]
        control: Option<u8>,
        #[serde(default)]
        value: Option<u16>,
    },
    /// Local / Usb / Both for clip playback and kaoss documentation.
    EmitMode {
        target: String,
        mode: String,
    },
    /// Drop and reopen the ALSA/cpal output stream without resetting engine state.
    AudioReopen,
}

const fn default_velocity() -> u8 {
    110
}

const fn default_drum_channel() -> u8 {
    9
}

const fn default_repeat_division() -> WireRepeatDivision {
    WireRepeatDivision::Quarter
}

const fn default_kaoss_root() -> u8 {
    48
}

const fn default_kaoss_octaves() -> u8 {
    2
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FxTargetSpec {
    Voice { index: u16 },
    Drum { index: u8 },
    DrumGroup,
    Bus,
}

impl From<FxTargetSpec> for FxTarget {
    fn from(spec: FxTargetSpec) -> Self {
        match spec {
            FxTargetSpec::Voice { index } => FxTarget::Voice(index),
            FxTargetSpec::Drum { index } => FxTarget::Drum(index),
            FxTargetSpec::DrumGroup => FxTarget::DrumGroup,
            FxTargetSpec::Bus => FxTarget::Bus,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireClipEvent {
    pub tick: u32,
    /// true = note on, false = note off.
    pub on: bool,
    pub channel: u8,
    pub note: u8,
    #[serde(default)]
    pub velocity: u8,
}

impl From<WireClipEvent> for ClipEvent {
    fn from(e: WireClipEvent) -> Self {
        ClipEvent {
            tick: e.tick,
            kind: if e.on {
                ClipEventKind::NoteOn {
                    channel: e.channel,
                    note: e.note,
                    velocity: e.velocity.max(1),
                }
            } else {
                ClipEventKind::NoteOff {
                    channel: e.channel,
                    note: e.note,
                }
            },
        }
    }
}

/// One reply to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Ok,
    Hello(HelloReply),
    Error { message: String },
    Status(StatusReply),
    /// Unsolicited: a MIDI event the engine heard (notes already went to DSP).
    Midi(MidiNotice),
}

/// UI-facing MIDI echo. DSP already applied the mapped command.
#[derive(Debug, Clone, Serialize)]
pub struct MidiNotice {
    pub kind: String,
    pub channel: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub velocity: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<u16>,
}

impl MidiNotice {
    pub fn from_event(event: midi_core::MidiEvent) -> Self {
        use midi_core::MidiEvent::*;
        match event {
            NoteOn {
                channel,
                note,
                velocity,
            } => Self {
                kind: "note_on".into(),
                channel,
                note: Some(note),
                velocity: Some(velocity as u16),
                control: None,
                value: None,
            },
            NoteOff {
                channel,
                note,
                velocity,
            } => Self {
                kind: "note_off".into(),
                channel,
                note: Some(note),
                velocity: Some(velocity as u16),
                control: None,
                value: None,
            },
            ControlChange {
                channel,
                controller,
                value,
            } => Self {
                kind: "control_change".into(),
                channel,
                note: None,
                velocity: None,
                control: Some(controller),
                value: Some(value as u16),
            },
            PitchBend { channel, value } => Self {
                kind: "pitch_bend".into(),
                channel,
                note: None,
                velocity: None,
                control: None,
                value: Some(value),
            },
            PolyPressure {
                channel,
                note,
                pressure,
            } => Self {
                kind: "poly_pressure".into(),
                channel,
                note: Some(note),
                velocity: None,
                control: None,
                value: Some(pressure as u16),
            },
            ChannelPressure { channel, pressure } => Self {
                kind: "channel_pressure".into(),
                channel,
                note: None,
                velocity: None,
                control: None,
                value: Some(pressure as u16),
            },
            ProgramChange { channel, program } => Self {
                kind: "program_change".into(),
                channel,
                note: None,
                velocity: None,
                control: None,
                value: Some(program as u16),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct StatusReply {
    pub position: u64,
    pub bpm: f32,
    pub active_voices: u16,
    pub active_drums: u16,
    pub active_repeats: u16,
    pub playing_clips: u16,
    pub peak: f32,
    pub callback_frames: u32,
    pub callback_micros: u32,
    pub callback_peak_micros: u32,
    /// Audio callback/backend errors or deadline misses.
    pub xruns: u64,
    pub command_drops: u64,
    pub emergency_releases: u64,
    pub touch_overwrites: u64,
}

/// What the control thread decides to do with a request.
pub enum Decoded {
    Hello {
        protocol: u16,
        client: String,
        realtime_owner: bool,
    },
    /// Plain command for the audio thread.
    Command(Command),
    /// Clip swap — allocated here, freed here; the audio thread only moves a pointer.
    ClipUpdate {
        slot: u8,
        clip: Option<Box<Clip>>,
        mode: Option<LaunchMode>,
    },
    /// Answer without touching audio.
    StatusRequest,
    /// Same path as USB MIDI in (notes, CC, bend).
    MidiIn(midi_core::MidiEvent),
    /// UI mode buttons: keys / drums / fx.
    KnobMap {
        mode: String,
        fx_kind: Option<String>,
        fx_index: u16,
    },
    Touch {
        gesture: u32,
        phase: TouchPhase,
        x: f32,
        y: f32,
        channel: u8,
        velocity: u8,
    },
    Repeat {
        gesture: u32,
        phase: RepeatPhase,
        note: u8,
        channel: u8,
        velocity: u8,
        division: RepeatDivision,
    },
    /// Rebuild the host audio device stream (engine + rings stay up).
    AudioReopen,
}

pub fn parse_quantize(value: Option<&str>) -> Quantize {
    match value.unwrap_or("off") {
        "bar" => Quantize::Bar,
        "beat" => Quantize::Beat,
        _ => Quantize::Off,
    }
}

pub fn parse_mode(value: &str) -> LaunchMode {
    match value {
        "one_shot" | "oneshot" | "one-shot" => LaunchMode::OneShot,
        _ => LaunchMode::Loop,
    }
}

fn parse_synth_param(name: &str) -> Option<SynthParam> {
    Some(match name {
        "morph" => SynthParam::Morph,
        "tone" => SynthParam::Tone,
        "level" => SynthParam::Level,
        "attack" => SynthParam::Attack,
        "release" => SynthParam::Release,
        "vibrato_depth" => SynthParam::VibratoDepth,
        "vibrato_rate" => SynthParam::VibratoRate,
        "vibrato_mod" => SynthParam::VibratoMod,
        "vibrato_always" => SynthParam::VibratoAlways,
        "pitch_bend" => SynthParam::PitchBend,
        "drum_pitch" => SynthParam::DrumPitch,
        "drum_decay" => SynthParam::DrumDecay,
        "drum_noise" => SynthParam::DrumNoise,
        "drum_tone" => SynthParam::DrumTone,
        "drum_level" => SynthParam::DrumLevel,
        _ => return None,
    })
}

fn midi_event_from_parts(
    kind: &str,
    channel: u8,
    note: Option<u8>,
    velocity: Option<u8>,
    control: Option<u8>,
    value: Option<u16>,
) -> Result<midi_core::MidiEvent, String> {
    let channel = channel & 0x0f;
    match kind {
        "note_on" => Ok(midi_core::MidiEvent::NoteOn {
            channel,
            note: note.unwrap_or(0) & 0x7f,
            velocity: velocity.unwrap_or(0).min(127),
        }),
        "note_off" => Ok(midi_core::MidiEvent::NoteOff {
            channel,
            note: note.unwrap_or(0) & 0x7f,
            velocity: velocity.unwrap_or(0).min(127),
        }),
        "control_change" | "cc" => Ok(midi_core::MidiEvent::ControlChange {
            channel,
            controller: control.unwrap_or(0) & 0x7f,
            value: (value.unwrap_or(0) as u8) & 0x7f,
        }),
        "pitch_bend" | "pitchwheel" => Ok(midi_core::MidiEvent::PitchBend {
            channel,
            value: value.unwrap_or(8192).min(16383),
        }),
        "channel_pressure" | "aftertouch" => Ok(midi_core::MidiEvent::ChannelPressure {
            channel,
            pressure: (value.unwrap_or(0) as u8) & 0x7f,
        }),
        "poly_pressure" | "polytouch" => Ok(midi_core::MidiEvent::PolyPressure {
            channel,
            note: note.unwrap_or(0) & 0x7f,
            pressure: (value.unwrap_or(0) as u8) & 0x7f,
        }),
        "program_change" => Ok(midi_core::MidiEvent::ProgramChange {
            channel,
            program: (value.unwrap_or(0) as u8) & 0x7f,
        }),
        other => Err(format!("unknown midi kind {other}")),
    }
}

fn parse_fx_param(name: &str) -> Option<FxParam> {
    Some(match name {
        "drive" => FxParam::Drive,
        "delay_time" => FxParam::DelayTime,
        "delay_fb" => FxParam::DelayFb,
        "delay_mix" => FxParam::DelayMix,
        "reverb_size" => FxParam::ReverbSize,
        "reverb_mix" => FxParam::ReverbMix,
        "flanger_mix" => FxParam::FlangerMix,
        "flanger_rate" => FxParam::FlangerRate,
        "flanger_depth" => FxParam::FlangerDepth,
        "flanger_fb" => FxParam::FlangerFb,
        _ => return None,
    })
}

/// Turn a parsed request into work. Allocation happens here, never in audio.
pub fn decode(request: Request) -> Result<Decoded, String> {
    Ok(match request {
        Request::NoteOn {
            channel,
            note,
            velocity,
        } => Decoded::MidiIn(midi_core::MidiEvent::NoteOn {
            channel: channel & 0x0f,
            note: note & 0x7f,
            velocity: velocity.min(127),
        }),
        Request::NoteOff { channel, note } => Decoded::MidiIn(midi_core::MidiEvent::NoteOff {
            channel: channel & 0x0f,
            note: note & 0x7f,
            velocity: 0,
        }),
        Request::Midi {
            kind,
            channel,
            note,
            velocity,
            control,
            value,
        } => Decoded::MidiIn(midi_event_from_parts(
            &kind, channel, note, velocity, control, value,
        )?),
        Request::KnobMap {
            mode,
            fx_kind,
            fx_index,
        } => Decoded::KnobMap {
            mode,
            fx_kind,
            fx_index,
        },
        Request::AllNotesOff => Decoded::Command(Command::AllNotesOff),
        Request::Panic => Decoded::Command(Command::Panic),
        Request::Synth { param, value } => {
            let param =
                parse_synth_param(&param).ok_or_else(|| format!("unknown param {param}"))?;
            Decoded::Command(Command::SetSynth { param, value })
        }
        Request::Fx {
            target,
            param,
            value,
        } => {
            let param =
                parse_fx_param(&param).ok_or_else(|| format!("unknown fx param {param}"))?;
            Decoded::Command(Command::SetFx {
                target: target.into(),
                param,
                value,
            })
        }
        Request::MorphPair { a, b } => Decoded::Command(Command::SetMorphPair { a, b }),
        Request::Tempo { bpm } => Decoded::Command(Command::SetTempo { bpm }),
        Request::BeatsPerBar { beats } => Decoded::Command(Command::SetBeatsPerBar { beats }),
        Request::ClipLoad {
            slot,
            length_ticks,
            mode,
            events,
        } => {
            let events: Vec<ClipEvent> = events.into_iter().map(ClipEvent::from).collect();
            Decoded::ClipUpdate {
                slot,
                clip: Some(Box::new(Clip::new(events, length_ticks))),
                mode: mode.as_deref().map(parse_mode),
            }
        }
        Request::ClipClear { slot } => Decoded::ClipUpdate {
            slot,
            clip: None,
            mode: None,
        },
        Request::ClipMode { slot, mode } => Decoded::Command(Command::SetClipMode {
            slot,
            mode: parse_mode(&mode),
        }),
        Request::ClipLaunch { slot, quantize } => Decoded::Command(Command::LaunchClip {
            slot,
            quantize: parse_quantize(quantize.as_deref()),
        }),
        Request::ClipStop { slot, quantize } => Decoded::Command(Command::StopClip {
            slot,
            quantize: parse_quantize(quantize.as_deref()),
        }),
        Request::StopAllClips => Decoded::Command(Command::StopAllClips),
        Request::Status => Decoded::StatusRequest,
        Request::KaossScale {
            scale_index,
            key,
            root_midi,
            octaves,
        } => Decoded::Command(Command::SetKaossScale {
            scale_index,
            key,
            root_midi,
            octaves,
        }),
        Request::MidiEmit {
            kind,
            channel,
            note,
            velocity,
            control,
            value,
        } => {
            let ev = midi_event_from_parts(
                &kind, channel, note, velocity, control, value,
            )?;
            let mut buf = [0u8; 3];
            let n = ev.encode(&mut buf);
            Decoded::Command(Command::MidiEmit {
                status: buf[0],
                d1: if n > 1 { buf[1] } else { 0 },
                d2: if n > 2 { buf[2] } else { 0 },
            })
        }
        Request::EmitMode { target, mode } => {
            let target = match target.to_ascii_lowercase().as_str() {
                "kaoss" => 1u8,
                _ => 0u8,
            };
            let mode = match mode.to_ascii_lowercase().as_str() {
                "usb" => 1u8,
                "both" => 2u8,
                _ => 0u8,
            };
            Decoded::Command(Command::SetEmitMode { target, mode })
        }
        Request::Hello {
            protocol,
            client,
            realtime_owner,
        } => Decoded::Hello {
            protocol,
            client,
            realtime_owner,
        },
        Request::Touch {
            gesture,
            phase,
            x,
            y,
            channel,
            velocity,
        } => Decoded::Touch {
            gesture,
            phase,
            x,
            y,
            channel,
            velocity,
        },
        Request::Repeat {
            gesture,
            phase,
            note,
            channel,
            velocity,
            division,
        } => Decoded::Repeat {
            gesture,
            phase,
            note,
            channel,
            velocity,
            division: map_repeat_division(division),
        },
        Request::AudioReopen => Decoded::AudioReopen,
    })
}

fn map_repeat_division(division: WireRepeatDivision) -> RepeatDivision {
    match division {
        WireRepeatDivision::Quarter => RepeatDivision::Quarter,
        WireRepeatDivision::Eighth => RepeatDivision::Eighth,
        WireRepeatDivision::EighthTriplet => RepeatDivision::EighthTriplet,
        WireRepeatDivision::Sixteenth => RepeatDivision::Sixteenth,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_line(line: &str) -> Decoded {
        let request: Request = serde_json::from_str(line).expect("parse");
        decode(request).expect("decode")
    }

    #[test]
    fn note_on_round_trips_from_json() {
        let d = decode_line(r#"{"cmd":"note_on","channel":0,"note":60,"velocity":100}"#);
        match d {
            Decoded::MidiIn(midi_core::MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            }) => {
                assert_eq!((channel, note, velocity), (0, 60, 100));
            }
            _ => panic!("wrong decode"),
        }
    }

    #[test]
    fn channel_and_note_are_masked_to_midi_range() {
        let d = decode_line(r#"{"cmd":"note_on","channel":99,"note":200,"velocity":250}"#);
        match d {
            Decoded::MidiIn(midi_core::MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            }) => {
                assert!(channel <= 15 && note <= 127 && velocity <= 127);
            }
            _ => panic!("wrong decode"),
        }
    }

    #[test]
    fn fx_target_selects_the_right_insert() {
        let d = decode_line(
            r#"{"cmd":"fx","target":{"kind":"drum","index":3},"param":"delay_mix","value":0.5}"#,
        );
        match d {
            Decoded::Command(Command::SetFx { target, param, .. }) => {
                assert_eq!(target, FxTarget::Drum(3));
                assert_eq!(param, FxParam::DelayMix);
            }
            _ => panic!("wrong decode"),
        }
    }

    #[test]
    fn clip_load_allocates_off_the_audio_thread() {
        let d = decode_line(
            r#"{"cmd":"clip_load","slot":2,"length_ticks":3840,"mode":"loop",
                "events":[{"tick":0,"on":true,"channel":9,"note":36,"velocity":110}]}"#,
        );
        match d {
            Decoded::ClipUpdate { slot, clip, mode } => {
                assert_eq!(slot, 2);
                assert_eq!(mode, Some(LaunchMode::Loop));
                let clip = clip.expect("clip");
                assert_eq!(clip.events().len(), 1);
                assert_eq!(clip.length_ticks(), 3840);
            }
            _ => panic!("wrong decode"),
        }
    }

    #[test]
    fn unknown_param_is_reported_not_panicked() {
        let request: Request =
            serde_json::from_str(r#"{"cmd":"synth","param":"nope","value":1.0}"#).unwrap();
        assert!(decode(request).is_err());
    }

    #[test]
    fn quantize_defaults_to_off() {
        assert_eq!(parse_quantize(None), Quantize::Off);
        assert_eq!(parse_quantize(Some("bar")), Quantize::Bar);
        assert_eq!(parse_quantize(Some("beat")), Quantize::Beat);
    }

    #[test]
    fn status_reply_serializes_for_the_ui() {
        let json = serde_json::to_string(&Response::Status(StatusReply {
            bpm: 120.0,
            ..Default::default()
        }))
        .unwrap();
        assert!(json.contains("bpm"));
    }

    #[test]
    fn midi_notice_is_externally_tagged() {
        let json = serde_json::to_string(&Response::Midi(MidiNotice::from_event(
            midi_core::MidiEvent::ControlChange {
                channel: 0,
                controller: 71,
                value: 64,
            },
        )))
        .unwrap();
        assert!(json.contains("\"midi\""));
        assert!(json.contains("control_change"));
        assert!(json.contains("71"));
    }

    #[test]
    fn hello_is_a_handshake_not_a_command() {
        let d = decode_line(r#"{"cmd":"hello","protocol":1,"client":"pidi-native"}"#);
        match d {
            Decoded::Hello {
                protocol,
                realtime_owner,
                ..
            } => {
                assert_eq!(protocol, 1);
                assert!(!realtime_owner);
            }
            _ => panic!("wrong decode"),
        }
    }

    #[test]
    fn touch_and_repeat_decode_from_json() {
        let d = decode_line(
            r#"{"cmd":"touch","gesture":41,"phase":"down","x":0.1,"y":0.9}"#,
        );
        assert!(matches!(d, Decoded::Touch { gesture: 41, .. }));
        let d = decode_line(r#"{"cmd":"repeat","gesture":7,"phase":"down","note":36}"#);
        match d {
            Decoded::Repeat {
                note,
                channel,
                division,
                ..
            } => {
                assert_eq!(note, 36);
                assert_eq!(channel, 9);
                assert_eq!(division, RepeatDivision::Quarter);
            }
            _ => panic!("wrong decode"),
        }
    }

    #[test]
    fn audio_reopen_decodes() {
        let d = decode_line(r#"{"cmd":"audio_reopen"}"#);
        assert!(matches!(d, Decoded::AudioReopen));
    }
}
