//! Line-delimited JSON control protocol.
//!
//! The UI speaks this over a socket. Parsing happens on the control thread; the
//! audio thread only ever sees the decoded [`Command`] (plain `Copy` data) or a
//! preallocated clip handed over as a `Box`.

use jambox_core::{
    Clip, ClipEvent, ClipEventKind, Command, FxParam, FxTarget, LaunchMode, Quantize, SynthParam,
};
use serde::{Deserialize, Serialize};

/// One request from the UI.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
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
    Error { message: String },
    Status(StatusReply),
}

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct StatusReply {
    pub position: u64,
    pub bpm: f32,
    pub active_voices: u16,
    pub active_drums: u16,
    pub playing_clips: u16,
    pub peak: f32,
    /// Blocks the audio callback could not fill in time.
    pub xruns: u64,
}

/// What the control thread decides to do with a request.
pub enum Decoded {
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
        "pitch_bend" => SynthParam::PitchBend,
        "drum_pitch" => SynthParam::DrumPitch,
        "drum_decay" => SynthParam::DrumDecay,
        "drum_noise" => SynthParam::DrumNoise,
        "drum_tone" => SynthParam::DrumTone,
        _ => return None,
    })
}

fn parse_fx_param(name: &str) -> Option<FxParam> {
    Some(match name {
        "drive" => FxParam::Drive,
        "delay_time" => FxParam::DelayTime,
        "delay_fb" => FxParam::DelayFb,
        "delay_mix" => FxParam::DelayMix,
        "reverb_size" => FxParam::ReverbSize,
        "reverb_mix" => FxParam::ReverbMix,
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
        } => Decoded::Command(Command::NoteOn {
            channel: channel & 0x0f,
            note: note & 0x7f,
            velocity: velocity.min(127),
        }),
        Request::NoteOff { channel, note } => Decoded::Command(Command::NoteOff {
            channel: channel & 0x0f,
            note: note & 0x7f,
        }),
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
    })
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
            Decoded::Command(Command::NoteOn {
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
            Decoded::Command(Command::NoteOn {
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
}
