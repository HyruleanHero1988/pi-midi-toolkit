//! The jambox engine: one `render` call per audio block.
//!
//! Ordering law: commands and clip events are resolved to a **frame**, the block is
//! split at those frames, and each span is rendered independently. A pad hit at
//! frame 300 of a 512-frame buffer is heard at frame 300 — not at the next boundary.

use midi_core::MidiEvent;

use crate::clip::{ClipEventKind, LaunchMode, SeqEvent, Sequencer};
use crate::command::{Command, FxParam, FxTarget, ScheduledCommand, SynthParam, MAX_BLOCK_COMMANDS};
use crate::drums::{drum_model_for_note, DrumKit, DrumMacros, DrumModel, DRUM_MODEL_COUNT, MAX_DRUM_HITS};
use crate::fx::{FxParams, FxUnit};
use crate::transport::Transport;
use crate::voice::{VoiceContext, VoicePool, MAX_VOICES};
use crate::wavetable::WaveBank;
use crate::DRUM_CHANNEL;

/// Max MIDI events the engine emits per block (clip playback → USB out).
pub const MAX_MIDI_OUT: usize = 128;
/// Largest audio block the engine preallocates for.
const MAX_BLOCK: usize = 2048;
/// Makeup gain before the soft limiter — Pi line/headphone out is timid.
const OUTPUT_MAKEUP: f32 = 1.65;

/// Frame-tagged MIDI produced by the engine (clip playback), for USB/DIN out.
pub struct MidiOutSink {
    events: [(u32, MidiEvent); MAX_MIDI_OUT],
    len: usize,
}

impl Default for MidiOutSink {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiOutSink {
    pub fn new() -> Self {
        Self {
            events: [(
                0,
                MidiEvent::ChannelPressure {
                    channel: 0,
                    pressure: 0,
                },
            ); MAX_MIDI_OUT],
            len: 0,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn push(&mut self, frame: u32, event: MidiEvent) {
        if self.len < MAX_MIDI_OUT {
            self.events[self.len] = (frame, event);
            self.len += 1;
        }
    }

    pub fn as_slice(&self) -> &[(u32, MidiEvent)] {
        &self.events[..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Cheap snapshot for the UI. Copied out after each block; never blocks audio.
#[derive(Debug, Clone, Copy, Default)]
pub struct EngineStatus {
    pub position: u64,
    pub bpm: f32,
    pub active_voices: u16,
    pub active_drums: u16,
    pub playing_clips: u16,
    pub peak: f32,
}

pub struct JamboxEngine {
    transport: Transport,
    bank: WaveBank,
    voices: VoicePool,
    drums: DrumKit,
    sequencer: Sequencer,

    voice_fx: Vec<FxUnit>,
    drum_fx: Vec<FxUnit>,
    drum_group_fx: FxUnit,
    bus_fx: FxUnit,

    key_bus: Vec<f32>,
    drum_bus: Vec<f32>,
    group_buf: Vec<f32>,
    seq_scratch: Vec<SeqEvent>,
    timeline: Vec<(u32, Command)>,

    tone: f32,
    level: f32,
    attack_sec: f32,
    release_sec: f32,
    vib_depth_semis: f32,
    vib_rate_hz: f32,
    vib_phase: f64,
    bend_semis: f32,
    tone_state: f32,
    status: EngineStatus,
}

impl JamboxEngine {
    pub fn new(sample_rate: f64) -> Self {
        let bank = WaveBank::with_builtins();
        let sr = sample_rate as f32;
        let voice_fx = (0..bank.len()).map(|_| FxUnit::new(sr)).collect();
        let drum_fx = (0..DRUM_MODEL_COUNT).map(|_| FxUnit::new(sr)).collect();
        Self {
            transport: Transport::new(sample_rate),
            bank,
            voices: VoicePool::new(),
            drums: DrumKit::new(sr),
            sequencer: Sequencer::new(),
            voice_fx,
            drum_fx,
            drum_group_fx: FxUnit::new(sr),
            bus_fx: FxUnit::new(sr),
            key_bus: vec![0.0; MAX_BLOCK],
            drum_bus: vec![0.0; MAX_BLOCK],
            group_buf: vec![0.0; MAX_BLOCK],
            seq_scratch: vec![
                SeqEvent {
                    frame: 0,
                    slot: 0,
                    kind: ClipEventKind::NoteOff {
                        channel: 0,
                        note: 0
                    },
                };
                MAX_BLOCK_COMMANDS
            ],
            timeline: Vec::with_capacity(MAX_BLOCK_COMMANDS * 2),
            tone: 1.0,
            level: 0.85,
            attack_sec: 0.012,
            release_sec: 0.030,
            vib_depth_semis: 0.0,
            vib_rate_hz: 5.0,
            vib_phase: 0.0,
            bend_semis: 0.0,
            tone_state: 0.0,
            status: EngineStatus::default(),
        }
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    pub fn bank(&self) -> &WaveBank {
        &self.bank
    }

    /// Host-thread access for loading wavetables / clips. Never call from audio.
    pub fn bank_mut(&mut self) -> &mut WaveBank {
        &mut self.bank
    }

    pub fn sequencer_mut(&mut self) -> &mut Sequencer {
        &mut self.sequencer
    }

    pub fn status(&self) -> EngineStatus {
        self.status
    }

    /// Make sure there is one FX insert per wavetable. Host thread (allocates).
    pub fn sync_fx_slots(&mut self) {
        let sr = self.transport.sample_rate() as f32;
        while self.voice_fx.len() < self.bank.len() {
            self.voice_fx.push(FxUnit::new(sr));
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        if (sample_rate - self.transport.sample_rate()).abs() < 1.0 {
            return;
        }
        self.transport.set_sample_rate(sample_rate);
        let sr = sample_rate as f32;
        self.drums.set_sample_rate(sr);
        for fx in self.voice_fx.iter_mut().chain(self.drum_fx.iter_mut()) {
            let params = fx.params();
            *fx = FxUnit::new(sr);
            fx.set_params(params);
        }
        let group = self.drum_group_fx.params();
        self.drum_group_fx = FxUnit::new(sr);
        self.drum_group_fx.set_params(group);
        let bus = self.bus_fx.params();
        self.bus_fx = FxUnit::new(sr);
        self.bus_fx.set_params(bus);
        self.voices.silence();
        self.drums.silence();
    }

    pub fn fx_params(&self, target: FxTarget) -> FxParams {
        match target {
            FxTarget::Voice(i) => self
                .voice_fx
                .get(i as usize)
                .map(|f| f.params())
                .unwrap_or_default(),
            FxTarget::Drum(i) => self
                .drum_fx
                .get(i as usize)
                .map(|f| f.params())
                .unwrap_or_default(),
            FxTarget::DrumGroup => self.drum_group_fx.params(),
            FxTarget::Bus => self.bus_fx.params(),
        }
    }

    /// Render one block. Allocation-free: every buffer was sized in `new`.
    pub fn render(
        &mut self,
        out: &mut [f32],
        commands: &[ScheduledCommand],
        midi_out: &mut MidiOutSink,
    ) {
        let frames = out.len().min(MAX_BLOCK);
        let out = &mut out[..frames];
        out.iter_mut().for_each(|s| *s = 0.0);
        midi_out.clear();
        if frames == 0 {
            return;
        }

        let block_start = self.transport.position();
        self.timeline.clear();

        for cmd in commands.iter().take(MAX_BLOCK_COMMANDS) {
            self.timeline
                .push((cmd.frame.min(frames as u32 - 1), cmd.command));
        }

        if self.transport.running() {
            let n = self.sequencer.collect(
                &self.transport,
                block_start,
                frames as u32,
                &mut self.seq_scratch,
            );
            for i in 0..n {
                let ev = self.seq_scratch[i];
                let frame = ev.frame.min(frames as u32 - 1);
                let (command, midi) = match ev.kind {
                    ClipEventKind::NoteOn {
                        channel,
                        note,
                        velocity,
                    } => (
                        Command::NoteOn {
                            channel,
                            note,
                            velocity,
                        },
                        MidiEvent::NoteOn {
                            channel,
                            note,
                            velocity,
                        },
                    ),
                    ClipEventKind::NoteOff { channel, note } => (
                        Command::NoteOff { channel, note },
                        MidiEvent::NoteOff {
                            channel,
                            note,
                            velocity: 0,
                        },
                    ),
                };
                self.timeline.push((frame, command));
                midi_out.push(frame, midi);
            }
        }

        self.timeline.sort_by_key(|(frame, _)| *frame);

        // Walk the block, splitting at every event frame.
        let mut cursor = 0usize;
        let mut next = 0usize;
        while cursor < frames {
            let boundary = self
                .timeline
                .get(next)
                .map(|(f, _)| (*f as usize).min(frames))
                .unwrap_or(frames);

            if boundary > cursor {
                self.render_span(&mut out[cursor..boundary]);
                cursor = boundary;
            }

            while next < self.timeline.len()
                && (self.timeline[next].0 as usize).min(frames) <= cursor
            {
                let command = self.timeline[next].1;
                self.apply(command);
                next += 1;
            }

            if next >= self.timeline.len() && cursor < frames {
                self.render_span(&mut out[cursor..frames]);
                cursor = frames;
            }
        }

        self.transport.advance(frames as u64);

        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        self.status = EngineStatus {
            position: self.transport.position(),
            bpm: self.transport.bpm() as f32,
            active_voices: self.voices.active_count() as u16,
            active_drums: self.drums.active_count() as u16,
            playing_clips: self.sequencer.playing_count() as u16,
            peak,
        };
    }

    /// Render one span with the current parameter set. No events happen inside.
    fn render_span(&mut self, out: &mut [f32]) {
        let n = out.len();
        if n == 0 {
            return;
        }
        let Self {
            transport,
            bank,
            voices,
            drums,
            voice_fx,
            drum_fx,
            drum_group_fx,
            bus_fx,
            key_bus,
            drum_bus,
            group_buf,
            tone,
            level,
            attack_sec,
            release_sec,
            vib_depth_semis,
            vib_rate_hz,
            vib_phase,
            bend_semis,
            tone_state,
            ..
        } = self;

        let sr = transport.sample_rate() as f32;
        key_bus[..n].iter_mut().for_each(|s| *s = 0.0);
        drum_bus[..n].iter_mut().for_each(|s| *s = 0.0);

        // Vibrato is per-span so long blocks still modulate smoothly.
        let vib = (*vib_phase).sin() as f32 * *vib_depth_semis;
        *vib_phase += std::f64::consts::TAU * *vib_rate_hz as f64 * n as f64 / sr as f64;
        if *vib_phase > std::f64::consts::TAU {
            *vib_phase -= std::f64::consts::TAU;
        }

        bank.rebuild_morph();
        let ctx = VoiceContext {
            sample_rate: sr,
            pitch_mul: 2f32.powf((*bend_semis + vib) / 12.0),
            attack_sec: *attack_sec,
            release_sec: *release_sec,
        };

        // Keys: one FX insert per wavetable group.
        let mut groups = [0usize; MAX_VOICES];
        let group_count = voices.active_groups(&mut groups);
        for &group in groups.iter().take(group_count) {
            group_buf[..n].iter_mut().for_each(|s| *s = 0.0);
            let table = if group == bank.nearer_index() && bank.morph_pair().0 != bank.morph_pair().1
            {
                bank.morph_table()
            } else {
                bank.table(group)
            };
            voices.render_group(group, table, &mut group_buf[..n], ctx);
            if let Some(fx) = voice_fx.get_mut(group) {
                if !fx.params().is_bypassed() {
                    fx.process(&mut group_buf[..n]);
                }
            }
            for i in 0..n {
                key_bus[i] += group_buf[i];
            }
        }

        // Tone (brightness) is a keys-only filter; drums have their own tone macro.
        if *tone < 0.999 {
            let coef = (0.02 + 0.98 * *tone).clamp(0.02, 1.0);
            for s in key_bus[..n].iter_mut() {
                *tone_state += (*s - *tone_state) * coef;
                *s = *tone_state;
            }
        } else if n > 0 {
            *tone_state = key_bus[n - 1];
        }

        // Drums: per-model insert, then the shared kit bus.
        let mut models = [0usize; MAX_DRUM_HITS];
        let model_count = drums.active_models(&mut models);
        for &model_idx in models.iter().take(model_count) {
            group_buf[..n].iter_mut().for_each(|s| *s = 0.0);
            drums.render_model(DrumModel::from_index(model_idx), &mut group_buf[..n]);
            if let Some(fx) = drum_fx.get_mut(model_idx) {
                if !fx.params().is_bypassed() {
                    fx.process(&mut group_buf[..n]);
                }
            }
            for i in 0..n {
                drum_bus[i] += group_buf[i];
            }
        }
        if !drum_group_fx.params().is_bypassed() {
            drum_group_fx.process(&mut drum_bus[..n]);
        }

        for i in 0..n {
            out[i] = key_bus[i] + drum_bus[i];
        }
        if !bus_fx.params().is_bypassed() {
            bus_fx.process(out);
        }

        let gain = *level * OUTPUT_MAKEUP;
        for s in out.iter_mut() {
            *s = (*s * gain).tanh() * 0.97;
        }
    }

    fn apply(&mut self, command: Command) {
        match command {
            Command::NoteOn {
                channel,
                note,
                velocity,
            } => {
                if velocity == 0 {
                    self.voices.note_off(channel, note);
                } else if channel == DRUM_CHANNEL {
                    self.drums.trigger(drum_model_for_note(note), velocity);
                } else {
                    let group = self.bank.nearer_index();
                    self.voices.note_on(channel, note, velocity, group);
                }
            }
            Command::NoteOff { channel, note } => {
                if channel != DRUM_CHANNEL {
                    self.voices.note_off(channel, note);
                }
            }
            Command::AllNotesOff => self.voices.all_notes_off(),
            Command::Panic => {
                self.voices.silence();
                self.drums.silence();
                let mut flush = [SeqEvent {
                    frame: 0,
                    slot: 0,
                    kind: ClipEventKind::NoteOff {
                        channel: 0,
                        note: 0,
                    },
                }; MAX_BLOCK_COMMANDS];
                self.sequencer.stop_all(&mut flush);
                for fx in self.voice_fx.iter_mut().chain(self.drum_fx.iter_mut()) {
                    fx.reset();
                }
                self.drum_group_fx.reset();
                self.bus_fx.reset();
            }
            Command::SetSynth { param, value } => self.set_synth(param, value),
            Command::SetFx {
                target,
                param,
                value,
            } => self.set_fx(target, param, value),
            Command::SetMorphPair { a, b } => {
                self.bank.set_morph_pair(a as usize, b as usize);
            }
            Command::SetTempo { bpm } => self.transport.set_bpm(bpm as f64),
            Command::SetBeatsPerBar { beats } => {
                self.transport.set_beats_per_bar(beats as u32);
            }
            Command::LaunchClip { slot, quantize } => {
                let now = self.transport.position();
                self.sequencer
                    .launch(slot as usize, now, quantize, &self.transport);
            }
            Command::StopClip { slot, quantize } => {
                let now = self.transport.position();
                self.sequencer
                    .stop(slot as usize, now, quantize, &self.transport);
            }
            Command::StopAllClips => {
                let mut flush = [SeqEvent {
                    frame: 0,
                    slot: 0,
                    kind: ClipEventKind::NoteOff {
                        channel: 0,
                        note: 0,
                    },
                }; MAX_BLOCK_COMMANDS];
                let n = self.sequencer.stop_all(&mut flush);
                for ev in flush.iter().take(n) {
                    if let ClipEventKind::NoteOff { channel, note } = ev.kind {
                        self.voices.note_off(channel, note);
                    }
                }
            }
            Command::SetClipMode { slot, mode } => {
                if let Some(s) = self.sequencer.slot_mut(slot as usize) {
                    s.set_mode(mode);
                }
            }
        }
    }

    fn set_synth(&mut self, param: SynthParam, value: f32) {
        let unit = value.clamp(0.0, 1.0);
        let mut macros = self.drums.macros();
        match param {
            SynthParam::Morph => self.bank.set_morph(unit),
            SynthParam::Tone => self.tone = unit,
            SynthParam::Level => self.level = unit,
            SynthParam::Attack => self.attack_sec = 0.002 + unit * 0.398,
            SynthParam::Release => self.release_sec = 0.010 + unit * 0.790,
            SynthParam::VibratoDepth => self.vib_depth_semis = unit * 1.0,
            SynthParam::VibratoRate => self.vib_rate_hz = 0.5 + unit * 12.0,
            SynthParam::PitchBend => self.bend_semis = value.clamp(-24.0, 24.0),
            SynthParam::DrumPitch => {
                macros.pitch = unit;
                self.drums.set_macros(macros);
            }
            SynthParam::DrumDecay => {
                macros.decay = unit;
                self.drums.set_macros(macros);
            }
            SynthParam::DrumNoise => {
                macros.noise = unit;
                self.drums.set_macros(macros);
            }
            SynthParam::DrumTone => {
                macros.tone = unit;
                self.drums.set_macros(macros);
            }
        }
    }

    fn set_fx(&mut self, target: FxTarget, param: FxParam, value: f32) {
        let unit = value.clamp(0.0, 1.0);
        let slot = match target {
            FxTarget::Voice(i) => self.voice_fx.get_mut(i as usize),
            FxTarget::Drum(i) => self.drum_fx.get_mut(i as usize),
            FxTarget::DrumGroup => Some(&mut self.drum_group_fx),
            FxTarget::Bus => Some(&mut self.bus_fx),
        };
        let Some(slot) = slot else { return };
        let mut params = slot.params();
        match param {
            FxParam::Drive => params.drive = unit,
            FxParam::DelayTime => params.delay_time = unit,
            FxParam::DelayFb => params.delay_fb = unit,
            FxParam::DelayMix => params.delay_mix = unit,
            FxParam::ReverbSize => params.reverb_size = unit,
            FxParam::ReverbMix => params.reverb_mix = unit,
        }
        slot.set_params(params);
    }

    /// Drum macros, for status / UI mirroring.
    pub fn drum_macros(&self) -> DrumMacros {
        self.drums.macros()
    }

    /// Clip slot mode, for status / UI mirroring.
    pub fn clip_mode(&self, slot: usize) -> Option<LaunchMode> {
        self.sequencer.slot(slot).map(|s| s.mode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Clip, ClipEvent};
    use crate::transport::{Quantize, PPQ};

    fn engine() -> JamboxEngine {
        let mut e = JamboxEngine::new(48_000.0);
        e.apply(Command::SetTempo { bpm: 120.0 });
        e
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = engine();
        let mut out = vec![0.0f32; 256];
        let mut midi = MidiOutSink::new();
        e.render(&mut out, &[], &mut midi);
        assert_eq!(peak(&out), 0.0);
        assert!(midi.is_empty());
    }

    #[test]
    fn a_note_makes_sound() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        let cmds = [ScheduledCommand::now(Command::NoteOn {
            channel: 0,
            note: 69,
            velocity: 120,
        })];
        e.render(&mut out, &cmds, &mut midi);
        assert!(peak(&out) > 0.01);
        assert_eq!(e.status().active_voices, 1);
    }

    #[test]
    fn a_mid_block_note_stays_silent_until_its_frame() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        let cmds = [ScheduledCommand {
            frame: 256,
            command: Command::NoteOn {
                channel: 0,
                note: 69,
                velocity: 127,
            },
        }];
        e.render(&mut out, &cmds, &mut midi);
        assert_eq!(peak(&out[..256]), 0.0, "sound before its frame");
        assert!(peak(&out[256..]) > 0.0, "no sound after its frame");
    }

    #[test]
    fn drum_channel_hits_the_kit_not_the_keys() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        let cmds = [ScheduledCommand::now(Command::NoteOn {
            channel: DRUM_CHANNEL,
            note: 36,
            velocity: 120,
        })];
        e.render(&mut out, &cmds, &mut midi);
        assert_eq!(e.status().active_voices, 0);
        assert_eq!(e.status().active_drums, 1);
        assert!(peak(&out) > 0.0);
    }

    #[test]
    fn clips_emit_midi_out_for_din() {
        let mut e = engine();
        let clip = Clip::new(
            vec![ClipEvent {
                tick: 0,
                kind: ClipEventKind::NoteOn {
                    channel: 0,
                    note: 60,
                    velocity: 100,
                },
            }],
            PPQ,
        );
        e.sequencer_mut().slot_mut(0).unwrap().set_clip(Some(clip));
        e.apply(Command::LaunchClip {
            slot: 0,
            quantize: Quantize::Off,
        });

        let mut out = vec![0.0f32; 256];
        let mut midi = MidiOutSink::new();
        e.render(&mut out, &[], &mut midi);
        assert_eq!(midi.len(), 1);
        assert!(matches!(midi.as_slice()[0].1, MidiEvent::NoteOn { .. }));
    }

    #[test]
    fn panic_kills_voices_and_clips() {
        let mut e = engine();
        let mut out = vec![0.0f32; 256];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::NoteOn {
                channel: 0,
                note: 60,
                velocity: 120,
            })],
            &mut midi,
        );
        assert_eq!(e.status().active_voices, 1);
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::Panic)],
            &mut midi,
        );
        assert_eq!(e.status().active_voices, 0);
        assert_eq!(peak(&out), 0.0);
    }

    #[test]
    fn transport_advances_by_the_block_size() {
        let mut e = engine();
        let mut out = vec![0.0f32; 128];
        let mut midi = MidiOutSink::new();
        e.render(&mut out, &[], &mut midi);
        e.render(&mut out, &[], &mut midi);
        assert_eq!(e.transport().position(), 256);
    }

    #[test]
    fn output_never_clips_the_dac() {
        let mut e = engine();
        let mut cmds = Vec::new();
        for note in 40..56u8 {
            cmds.push(ScheduledCommand::now(Command::NoteOn {
                channel: 0,
                note,
                velocity: 127,
            }));
        }
        cmds.push(ScheduledCommand::now(Command::SetSynth {
            param: SynthParam::Level,
            value: 1.0,
        }));
        let mut out = vec![0.0f32; 1024];
        let mut midi = MidiOutSink::new();
        for _ in 0..8 {
            e.render(&mut out, &cmds, &mut midi);
            assert!(peak(&out) <= 1.0, "soft limiter must hold the rails");
        }
    }

    #[test]
    fn fx_commands_reach_the_right_insert() {
        let mut e = engine();
        let mut out = vec![0.0f32; 64];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::SetFx {
                target: FxTarget::Drum(0),
                param: FxParam::DelayMix,
                value: 0.7,
            })],
            &mut midi,
        );
        assert!((e.fx_params(FxTarget::Drum(0)).delay_mix - 0.7).abs() < 1e-6);
        assert_eq!(e.fx_params(FxTarget::Bus).delay_mix, 0.0);
        assert_eq!(e.fx_params(FxTarget::Voice(0)).delay_mix, 0.0);
    }

    #[test]
    fn a_loop_keeps_time_while_commands_flood_in() {
        // The point of the rewrite: UI chatter must not move the beat.
        let mut e = engine();
        let clip = Clip::new(
            (0..4)
                .map(|beat| ClipEvent {
                    tick: beat * PPQ,
                    kind: ClipEventKind::NoteOn {
                        channel: DRUM_CHANNEL,
                        note: 36,
                        velocity: 110,
                    },
                })
                .collect(),
            PPQ * 4,
        );
        e.sequencer_mut().slot_mut(0).unwrap().set_clip(Some(clip));
        e.apply(Command::LaunchClip {
            slot: 0,
            quantize: Quantize::Off,
        });

        let beat = e.transport().samples_per_beat() as u64;
        let mut out = vec![0.0f32; 128];
        let mut midi = MidiOutSink::new();
        let mut hits = Vec::new();
        let mut pos = 0u64;
        while pos < beat * 8 {
            // A knob being dragged every single block.
            let noise = [
                ScheduledCommand::now(Command::SetSynth {
                    param: SynthParam::Morph,
                    value: (pos % 100) as f32 / 100.0,
                }),
                ScheduledCommand::now(Command::SetFx {
                    target: FxTarget::Bus,
                    param: FxParam::ReverbMix,
                    value: 0.1,
                }),
            ];
            e.render(&mut out, &noise, &mut midi);
            for (frame, _) in midi.as_slice() {
                hits.push(pos + *frame as u64);
            }
            pos += 128;
        }
        let expected: Vec<u64> = (0..8).map(|i| i * beat).collect();
        assert_eq!(hits, expected);
    }
}
