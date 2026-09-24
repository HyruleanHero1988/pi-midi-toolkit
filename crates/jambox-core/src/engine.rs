//! The jambox engine: one `render` call per audio block.
//!
//! Ordering law: commands and clip events are resolved to a **frame**, the block is
//! split at those frames, and each span is rendered independently. A pad hit at
//! frame 300 of a 512-frame buffer is heard at frame 300 — not at the next boundary.

use midi_core::MidiEvent;

use crate::clip::{
    Clip, ClipEventKind, ClipVoice, LaunchMode, SeqEvent, Sequencer, MAX_CLIPS, SEQ_KAOSS_MIX_SLOT,
};
use crate::mix::MixSource;
use crate::command::{
    Command, EmitMode, FxParam, FxTarget, ScheduledCommand, SynthParam, MAX_BLOCK_COMMANDS,
};
use crate::drums::{
    drum_model_for_note, DrumKit, DrumMacros, DrumModel, DRUM_MODEL_COUNT, MAX_DRUM_HITS,
};
use crate::fm::FmSynth;
use crate::fx::{FxParams, FxUnit};
use crate::arp::{
    ArpDivision, ArpEvent, ArpOrder, Arpeggiator, MAX_ARP_EVENTS_PER_BLOCK,
};
use crate::kaoss::{unpack_xy, KaossMapper, LatestTouch, TouchDelta};
use crate::repeat::{RepeatEvent, RepeatRack, MAX_REPEAT_EVENTS_PER_BLOCK};
use crate::transport::Transport;
use crate::voice::{VoiceContext, VoicePool, MAX_VOICES};
use crate::wavetable::{WaveBank, TABLE_SIZE};
use crate::DRUM_CHANNEL;

/// Max MIDI events the engine emits per block (clip playback → USB out).
pub const MAX_MIDI_OUT: usize = 128;
/// Largest audio block the engine preallocates for.
///
/// bcm2835 Headphones with cpal `BufferSize::Default` often callbacks at the
/// full ALSA buffer (~4410 frames at 44.1 kHz), not the period. Rendering only
/// 2048 of those left a silent tail every callback — choppy audio on the Pi.
pub const MAX_RENDER_BLOCK: usize = 8192;
const MAX_BLOCK: usize = MAX_RENDER_BLOCK;
/// Makeup gain before the soft limiter — Pi line/headphone out is timid.
const OUTPUT_MAKEUP: f32 = 1.65;
/// Extra kit bus gain, matching Python `DRUM_BUS_GAIN`.
const DRUM_BUS_GAIN: f32 = 1.55;

const ATTACK_SEC_MIN: f32 = 0.002;
const ATTACK_SEC_MAX: f32 = 0.400;
const RELEASE_SEC_MIN: f32 = 0.010;
const RELEASE_SEC_MAX: f32 = 0.800;

/// Slow filter sweep at the bottom of the WAH pad.
pub const TONE_LFO_HZ_MIN: f32 = 0.3;
/// Fast auto-wah at the top of the WAH pad.
pub const TONE_LFO_HZ_MAX: f32 = 10.0;

/// Pad Y 0..1 → tone-LFO rate. Exponential so the lower half stays a slow sweep.
pub fn tone_lfo_hz_from_unit(unit: f32) -> f32 {
    let t = unit.clamp(0.0, 1.0);
    TONE_LFO_HZ_MIN * (TONE_LFO_HZ_MAX / TONE_LFO_HZ_MIN).powf(t)
}

/// Exponential knob → seconds, matching Python `set_attack` / `set_release`.
fn map_exp_time(unit: f32, min: f32, max: f32) -> f32 {
    min * (max / min).powf(unit.clamp(0.0, 1.0))
}

/// Resonant lowpass for keys brightness (Kaoss Y / MPK tone knob).
/// `tone` 0 = dark, 1 = open/bypass. Matches Python `apply_tone_lowpass`.
fn apply_tone_lowpass(buf: &mut [f32], tone: f32, lp: &mut f32, bp: &mut f32, sample_rate: f32) {
    if buf.is_empty() {
        return;
    }
    let tone = tone.clamp(0.0, 1.0);
    if tone >= 0.985 {
        *lp = buf[buf.len() - 1];
        *bp = 0.0;
        return;
    }
    let sr = sample_rate.max(8000.0);
    let fc = 90.0 * (8000.0_f32 / 90.0).powf(tone);
    let fc = fc.min(sr * 0.14);
    let f = (2.0 * std::f32::consts::PI * fc / sr).sin();
    let damp = 0.38 + 0.62 * tone;
    let mut l = *lp;
    let mut b = *bp;
    for s in buf.iter_mut() {
        l += f * b;
        let hp = *s - l - damp * b;
        b += f * hp;
        *s = l;
    }
    *lp = l;
    *bp = b;
}

/// Same SVF as [`apply_tone_lowpass`], with a per-sample sine on cutoff.
///
/// Live WAH now runs per voice; this helper stays as the coefficient reference.
#[allow(dead_code)]
fn apply_tone_lowpass_lfo(
    buf: &mut [f32],
    base_tone: f32,
    amount: f32,
    rate_hz: f32,
    phase: &mut f64,
    lp: &mut f32,
    bp: &mut f32,
    sample_rate: f32,
) {
    if buf.is_empty() {
        return;
    }
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.01 {
        apply_tone_lowpass(buf, base_tone, lp, bp, sample_rate);
        return;
    }
    let sr = sample_rate.max(8000.0);
    let phase_inc = std::f64::consts::TAU * rate_hz.max(0.01) as f64 / sr as f64;
    let base = base_tone.clamp(0.0, 1.0);
    let mut l = *lp;
    let mut b = *bp;
    for s in buf.iter_mut() {
        *phase += phase_inc;
        if *phase > std::f64::consts::TAU {
            *phase %= std::f64::consts::TAU;
        }
        let lfo = 0.5 + 0.5 * (*phase).sin() as f32;
        let tone = (base * (1.0 - amount) + lfo * amount).clamp(0.0, 1.0);
        if tone >= 0.985 {
            l = *s;
            b = 0.0;
            continue;
        }
        let fc = 90.0 * (8000.0_f32 / 90.0).powf(tone);
        let fc = fc.min(sr * 0.14);
        let f = (2.0 * std::f32::consts::PI * fc / sr).sin();
        let damp = 0.38 + 0.62 * tone;
        l += f * b;
        let hp = *s - l - damp * b;
        b += f * hp;
        *s = l;
    }
    *lp = l;
    *bp = b;
}

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
    pub active_repeats: u16,
    pub playing_clips: u16,
    pub peak: f32,
    pub arp_enabled: bool,
    pub arp_latched: bool,
    pub arp_root: u8,
    pub arp_step: u8,
}

pub struct JamboxEngine {
    transport: Transport,
    bank: WaveBank,
    voices: VoicePool,
    fm: FmSynth,
    fm_enabled: bool,
    drums: DrumKit,
    sequencer: Sequencer,
    repeats: RepeatRack,
    arp: Arpeggiator,
    kaoss: KaossMapper,
    /// Recorded SEQ Kaoss gestures — separate so live fingers cannot steal them.
    kaoss_seq: KaossMapper,
    kaoss_seq_mode: [u8; crate::kaoss::MAX_TOUCH_VOICES],

    voice_fx: Vec<FxUnit>,
    drum_fx: Vec<FxUnit>,
    drum_group_fx: FxUnit,
    bus_fx: FxUnit,
    clip_fx: Vec<FxUnit>,
    clip_table: [[f32; TABLE_SIZE]; MAX_CLIPS],
    clip_baked: [bool; MAX_CLIPS],

    key_bus: Vec<f32>,
    drum_bus: Vec<f32>,
    group_buf: Vec<f32>,
    seq_scratch: Vec<SeqEvent>,
    repeat_scratch: [RepeatEvent; MAX_REPEAT_EVENTS_PER_BLOCK],
    arp_scratch: [ArpEvent; MAX_ARP_EVENTS_PER_BLOCK],
    timeline: Vec<(u32, Command)>,

    tone: f32,
    level: f32,
    drum_level: f32,
    clip_gain: [f32; MAX_CLIPS],
    attack_sec: f32,
    release_sec: f32,
    vib_depth_semis: f32,
    vib_rate_hz: f32,
    vib_mod: f32,
    vib_always: f32,
    vib_phase: f64,
    tone_lfo_amount: f32,
    tone_lfo_rate_hz: f32,
    tone_lfo_phase: f64,
    bend_semis: f32,
    clip_emit: EmitMode,
    kaoss_emit: EmitMode,
    arp_emit: EmitMode,
    /// Last arp pitch the engine actually sounded — released before the next step.
    arp_live: Option<(u8, u8)>,
    status: EngineStatus,
}

/// Keep wet insert tanks moving after the last voice in a group goes idle,
/// so a strum's delay / reverb can finish instead of getting chopped off.
fn flush_fx_tails(
    units: &mut [FxUnit],
    already: &[usize],
    already_n: usize,
    extra_skip: Option<usize>,
    scratch: &mut [f32],
    dest: &mut [f32],
    n: usize,
) {
    for (i, fx) in units.iter_mut().enumerate() {
        if already[..already_n].contains(&i) || extra_skip == Some(i) {
            continue;
        }
        if !fx.has_tail() {
            continue;
        }
        scratch[..n].iter_mut().for_each(|s| *s = 0.0);
        fx.process(&mut scratch[..n]);
        for j in 0..n {
            dest[j] += scratch[j];
        }
    }
}

impl JamboxEngine {
    pub fn new(sample_rate: f64) -> Self {
        Self::with_bank(sample_rate, WaveBank::with_builtins())
    }

    /// Host thread: pass a preloaded wavetable bank (AKWF dir, etc.).
    pub fn with_bank(sample_rate: f64, bank: WaveBank) -> Self {
        let sr = sample_rate as f32;
        let voice_fx = (0..bank.len()).map(|_| FxUnit::new(sr)).collect();
        let drum_fx = (0..DRUM_MODEL_COUNT).map(|_| FxUnit::new(sr)).collect();
        Self {
            transport: Transport::new(sample_rate),
            bank,
            voices: VoicePool::new(),
            fm: FmSynth::new(),
            fm_enabled: false,
            drums: DrumKit::new(sr),
            sequencer: Sequencer::new(),
            repeats: RepeatRack::new(),
            arp: Arpeggiator::new(),
            kaoss: KaossMapper::new(),
            kaoss_seq: KaossMapper::new(),
            kaoss_seq_mode: [0; crate::kaoss::MAX_TOUCH_VOICES],
            voice_fx,
            drum_fx,
            drum_group_fx: FxUnit::new(sr),
            bus_fx: FxUnit::new(sr),
            clip_fx: (0..MAX_CLIPS).map(|_| FxUnit::new(sr)).collect(),
            clip_table: [[0.0; TABLE_SIZE]; MAX_CLIPS],
            clip_baked: [false; MAX_CLIPS],
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
            repeat_scratch: [RepeatEvent {
                frame: 0,
                owner: 0,
                channel: 0,
                note: 0,
                velocity: 0,
            }; MAX_REPEAT_EVENTS_PER_BLOCK],
            arp_scratch: [ArpEvent {
                frame: 0,
                channel: 0,
                note: 0,
                velocity: 0,
                on: false,
            }; MAX_ARP_EVENTS_PER_BLOCK],
            timeline: Vec::with_capacity(
                MAX_BLOCK_COMMANDS * 2 + MAX_REPEAT_EVENTS_PER_BLOCK + MAX_ARP_EVENTS_PER_BLOCK,
            ),
            tone: 1.0,
            level: 1.0,
            drum_level: 1.0,
            clip_gain: [1.0; MAX_CLIPS],
            attack_sec: 0.012,
            release_sec: 0.030,
            vib_depth_semis: 0.5,
            vib_rate_hz: 5.0,
            vib_mod: 0.0,
            vib_always: 0.0,
            vib_phase: 0.0,
            tone_lfo_amount: 0.0,
            tone_lfo_rate_hz: tone_lfo_hz_from_unit(0.5),
            tone_lfo_phase: 0.0,
            bend_semis: 0.0,
            clip_emit: EmitMode::Both,
            kaoss_emit: EmitMode::Local,
            arp_emit: EmitMode::Both,
            arp_live: None,
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

    /// Audio-thread clip pointer swap. Bakes a locked voice without allocating.
    pub fn apply_clip_update(
        &mut self,
        slot: usize,
        clip: Option<Box<Clip>>,
        mode: Option<LaunchMode>,
        tone: Option<f32>,
        voice: Option<ClipVoice>,
    ) -> Option<Box<Clip>> {
        let slot = slot.min(MAX_CLIPS - 1);
        if let Some(mode) = mode {
            if let Some(s) = self.sequencer.slot_mut(slot) {
                s.set_mode(mode);
            }
        }
        if let Some(voice) = voice {
            if let Some(s) = self.sequencer.slot_mut(slot) {
                s.set_voice(voice);
            }
            if voice.locked {
                self.bank.blend_pair(
                    voice.morph_a as usize,
                    voice.morph_b as usize,
                    voice.morph,
                    &mut self.clip_table[slot],
                );
                self.clip_baked[slot] = true;
                if let Some(fx) = self.clip_fx.get_mut(slot) {
                    fx.set_params(voice.fx_params());
                }
            } else {
                self.clip_baked[slot] = false;
            }
        } else if let Some(tone) = tone {
            if let Some(s) = self.sequencer.slot_mut(slot) {
                s.set_playback_tone(tone);
            }
        } else if clip.is_none() {
            if let Some(s) = self.sequencer.slot_mut(slot) {
                s.set_voice(ClipVoice::default());
            }
            self.clip_baked[slot] = false;
            if let Some(fx) = self.clip_fx.get_mut(slot) {
                fx.set_params(FxParams::default());
                fx.reset();
            }
        }
        self.sequencer
            .slot_mut(slot)
            .and_then(|s| s.swap_boxed(clip))
    }

    pub fn status(&self) -> EngineStatus {
        self.status
    }

    pub fn active_touches(&self) -> usize {
        self.kaoss.active_count()
    }

    pub fn clip_emit(&self) -> EmitMode {
        self.clip_emit
    }

    pub fn kaoss_emit(&self) -> EmitMode {
        self.kaoss_emit
    }

    pub fn arp_emit(&self) -> EmitMode {
        self.arp_emit
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
        for fx in self
            .voice_fx
            .iter_mut()
            .chain(self.drum_fx.iter_mut())
            .chain(self.clip_fx.iter_mut())
        {
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
        self.fm.silence();
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
                        Command::ClipNoteOn {
                            channel,
                            note,
                            velocity,
                            slot: ev.slot as u8,
                        },
                        Some(MidiEvent::NoteOn {
                            channel,
                            note,
                            velocity,
                        }),
                    ),
                    ClipEventKind::NoteOff { channel, note } => (
                        Command::ClipNoteOff { channel, note },
                        Some(MidiEvent::NoteOff {
                            channel,
                            note,
                            velocity: 0,
                        }),
                    ),
                    ClipEventKind::TouchDown { owner, x, y, mode } => (
                        Command::ClipTouchDown {
                            owner: owner as u32,
                            x,
                            y,
                            slot: ev.slot as u8,
                            mode,
                        },
                        None,
                    ),
                    ClipEventKind::TouchMove { owner, x, y } => (
                        Command::ClipTouchMove {
                            owner: owner as u32,
                            x,
                            y,
                            slot: ev.slot as u8,
                        },
                        None,
                    ),
                    ClipEventKind::TouchUp { owner } => (
                        Command::ClipTouchUp {
                            owner: owner as u32,
                            slot: ev.slot as u8,
                        },
                        None,
                    ),
                };
                if self.clip_emit.includes_local() {
                    self.timeline.push((frame, command));
                }
                if self.clip_emit.includes_usb() {
                    if let Some(midi) = midi {
                        midi_out.push(frame, midi);
                    }
                }
            }

            let repeat_count = self.repeats.collect(
                &self.transport,
                block_start,
                frames as u32,
                &mut self.repeat_scratch,
            );
            for event in self.repeat_scratch.iter().take(repeat_count) {
                self.timeline.push((
                    event.frame.min(frames as u32 - 1),
                    Command::RepeatHit {
                        owner: event.owner,
                        channel: event.channel,
                        note: event.note,
                        velocity: event.velocity,
                    },
                ));
            }

            let arp_count = self.arp.collect(
                &self.transport,
                block_start,
                frames as u32,
                &mut self.arp_scratch,
            );
            for event in self.arp_scratch.iter().take(arp_count) {
                self.timeline.push((
                    event.frame.min(frames as u32 - 1),
                    Command::ArpVoice {
                        channel: event.channel,
                        note: event.note,
                        velocity: event.velocity,
                        on: event.on,
                    },
                ));
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
                let absolute_frame = block_start.saturating_add(cursor as u64);
                self.apply(command, absolute_frame, cursor as u32, midi_out);
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
            active_voices: (self.voices.active_count() + self.fm.active_count()) as u16,
            active_drums: self.drums.active_count() as u16,
            active_repeats: self.repeats.active_count() as u16,
            playing_clips: self.sequencer.playing_count() as u16,
            peak,
            arp_enabled: self.arp.enabled(),
            arp_latched: self.arp.latched(),
            arp_root: self.arp.root(),
            arp_step: self.arp.cursor(),
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
            fm,
            fm_enabled,
            drums,
            sequencer,
            voice_fx,
            drum_fx,
            drum_group_fx,
            bus_fx,
            clip_fx,
            clip_table,
            clip_baked,
            key_bus,
            drum_bus,
            group_buf,
            tone,
            level,
            drum_level,
            clip_gain,
            attack_sec,
            release_sec,
            vib_depth_semis,
            vib_rate_hz,
            vib_mod,
            vib_always,
            vib_phase,
            tone_lfo_amount,
            tone_lfo_rate_hz,
            tone_lfo_phase,
            bend_semis,
            ..
        } = self;

        let sr = transport.sample_rate() as f32;
        key_bus[..n].iter_mut().for_each(|s| *s = 0.0);
        drum_bus[..n].iter_mut().for_each(|s| *s = 0.0);

        // Wheel or screen — whichever asks for more, matching Python `max(_mod, _vib_always)`.
        let vib_amt = (*vib_mod).max(*vib_always);
        let mut vib = 0.0f32;
        if vib_amt > 0.01 && *vib_depth_semis > 0.001 {
            *vib_phase += std::f64::consts::TAU * *vib_rate_hz as f64 * n as f64 / sr as f64;
            if *vib_phase > std::f64::consts::TAU {
                *vib_phase %= std::f64::consts::TAU;
            }
            vib = (*vib_phase).sin() as f32 * *vib_depth_semis * vib_amt;
        }

        let pitch_mul = 2f32.powf((*bend_semis + vib) / 12.0);
        bank.rebuild_morph();
        let ctx = VoiceContext {
            sample_rate: sr,
            pitch_mul,
            attack_sec: *attack_sec,
            release_sec: *release_sec,
            live_gain: *level,
            clip_gains: *clip_gain,
            live_tone: *tone,
            tone_lfo_amount: *tone_lfo_amount,
            tone_lfo_rate_hz: *tone_lfo_rate_hz,
            tone_lfo_phase: *tone_lfo_phase,
        };
        if *tone_lfo_amount > 0.01 {
            *tone_lfo_phase +=
                std::f64::consts::TAU * *tone_lfo_rate_hz as f64 * n as f64 / sr as f64;
            if *tone_lfo_phase > std::f64::consts::TAU {
                *tone_lfo_phase %= std::f64::consts::TAU;
            }
        }

        drums.set_mix_gains(*drum_level, *clip_gain);

        // Live keys: one FX insert per wavetable group.
        let mut groups = [0usize; MAX_VOICES];
        let group_count = voices.active_groups(&mut groups);
        for &group in groups.iter().take(group_count) {
            group_buf[..n].iter_mut().for_each(|s| *s = 0.0);
            let table = bank.table_for_live_group(group);
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

        // Recorded pads / SEQ: baked table + insert when locked, else live morph.
        let mut slots = [0usize; MAX_VOICES];
        let slot_count = voices.active_clip_slots(&mut slots);
        let mut used_clip_fx = [0usize; MAX_VOICES];
        let mut used_clip_fx_n = 0;
        let mut unlocked_voice_fx = None;
        for &slot in slots.iter().take(slot_count) {
            group_buf[..n].iter_mut().for_each(|s| *s = 0.0);
            let locked = clip_baked[slot]
                && sequencer
                    .slot(slot)
                    .map(|s| s.voice().locked)
                    .unwrap_or(false);
            let table = if locked {
                &clip_table[slot]
            } else {
                bank.morph_table()
            };
            voices.render_clip_slot(slot, table, &mut group_buf[..n], ctx);
            let fx = if locked {
                used_clip_fx[used_clip_fx_n] = slot;
                used_clip_fx_n += 1;
                clip_fx.get_mut(slot)
            } else {
                unlocked_voice_fx = Some(bank.nearer_index());
                voice_fx.get_mut(bank.nearer_index())
            };
            if let Some(fx) = fx {
                if !fx.params().is_bypassed() {
                    fx.process(&mut group_buf[..n]);
                }
            }
            for i in 0..n {
                key_bus[i] += group_buf[i];
            }
        }
        flush_fx_tails(
            voice_fx,
            &groups,
            group_count,
            unlocked_voice_fx,
            group_buf,
            key_bus,
            n,
        );
        flush_fx_tails(
            clip_fx,
            &used_clip_fx,
            used_clip_fx_n,
            None,
            group_buf,
            key_bus,
            n,
        );

        if *fm_enabled || fm.active_count() > 0 {
            group_buf[..n].iter_mut().for_each(|s| *s = 0.0);
            fm.render_split(&mut group_buf[..n], &mut [], ctx);
            for i in 0..n {
                key_bus[i] += group_buf[i];
            }
        }

        // Drums: per-model insert, then the shared kit bus.
        let mut models = [0usize; MAX_DRUM_HITS];
        let model_count = drums.active_models(&mut models);
        for &model_idx in models.iter().take(model_count) {
            group_buf[..n].iter_mut().for_each(|s| *s = 0.0);
            drums.render_model(DrumModel::from_index(model_idx), &mut group_buf[..n]);
            for s in group_buf[..n].iter_mut() {
                *s *= DRUM_BUS_GAIN;
            }
            if let Some(fx) = drum_fx.get_mut(model_idx) {
                if !fx.params().is_bypassed() {
                    fx.process(&mut group_buf[..n]);
                }
            }
            for i in 0..n {
                drum_bus[i] += group_buf[i];
            }
        }
        flush_fx_tails(drum_fx, &models, model_count, None, group_buf, drum_bus, n);
        if !drum_group_fx.params().is_bypassed() {
            drum_group_fx.process(&mut drum_bus[..n]);
        }

        // Keys `level` / kit `drum_level` / clip gains are applied at the voice.
        for i in 0..n {
            out[i] = key_bus[i] + drum_bus[i];
        }
        if !bus_fx.params().is_bypassed() {
            bus_fx.process(out);
        }

        for s in out.iter_mut() {
            *s = (*s * OUTPUT_MAKEUP).tanh() * 0.97;
        }
    }

    fn apply(
        &mut self,
        command: Command,
        absolute_frame: u64,
        relative_frame: u32,
        midi_out: &mut MidiOutSink,
    ) {
        match command {
            Command::NoteOn {
                channel,
                note,
                velocity,
            } => {
                if self.arp.enabled() && channel != DRUM_CHANNEL {
                    if velocity == 0 {
                        if let Some((ch, n)) = self.arp.note_off(note) {
                            self.apply_arp_voice(ch, n, 0, false, relative_frame, midi_out);
                        }
                    } else if self.arp.note_on(note, velocity, channel, absolute_frame) {
                        let step = self
                            .transport
                            .ticks_to_samples(self.arp.division().ticks())
                            .round()
                            .max(1.0) as u64;
                        if let Some((ch, n, vel)) = self.arp.take_attack(absolute_frame, step) {
                            self.apply_arp_voice(ch, n, vel, true, relative_frame, midi_out);
                        }
                    }
                } else {
                    self.sound_on(channel, note, velocity, MixSource::Live);
                }
            }
            Command::NoteOff { channel, note } => {
                if self.arp.enabled() && channel != DRUM_CHANNEL {
                    if let Some((ch, n)) = self.arp.note_off(note) {
                        self.apply_arp_voice(ch, n, 0, false, relative_frame, midi_out);
                    }
                } else if channel != DRUM_CHANNEL {
                    self.voices.note_off(channel, note);
                    self.fm.note_off(channel, note);
                }
            }
            Command::ClipNoteOn {
                channel,
                note,
                velocity,
                slot,
            } => {
                let mix = MixSource::seq_family(slot as usize, channel);
                let tone = self
                    .sequencer
                    .slot(slot as usize)
                    .map(|s| s.playback_tone())
                    .unwrap_or(1.0);
                if velocity == 0 {
                    self.voices.note_off_recorded(channel, note);
                    self.fm.note_off_recorded(channel, note);
                } else if channel == DRUM_CHANNEL {
                    self.drums
                        .trigger_mix(drum_model_for_note(note), velocity, mix);
                } else {
                    // Clips stay on the wavetable path so FM mode cannot rewrite a take.
                    let group = self.bank.nearer_index();
                    self.voices
                        .note_on_recorded(channel, note, velocity, group, tone, mix);
                }
            }
            Command::ClipNoteOff { channel, note } => {
                if channel != DRUM_CHANNEL {
                    self.voices.note_off_recorded(channel, note);
                    self.fm.note_off_recorded(channel, note);
                }
            }
            Command::AllNotesOff => {
                self.voices.all_notes_off();
                self.fm.all_notes_off();
                self.repeats.stop_all();
                if let Some((ch, n)) = self.arp.panic() {
                    self.apply_arp_voice(ch, n, 0, false, relative_frame, midi_out);
                }
                self.arp_live = None;
                self.release_kaoss();
            }
            Command::Panic => {
                self.voices.silence();
                self.fm.silence();
                self.drums.silence();
                self.repeats.stop_all();
                let _ = self.arp.panic();
                self.arp_live = None;
                self.release_kaoss();
                let mut flush = [SeqEvent {
                    frame: 0,
                    slot: 0,
                    kind: ClipEventKind::NoteOff {
                        channel: 0,
                        note: 0,
                    },
                }; MAX_BLOCK_COMMANDS];
                self.sequencer.stop_all(&mut flush);
                for fx in self
                    .voice_fx
                    .iter_mut()
                    .chain(self.drum_fx.iter_mut())
                    .chain(self.clip_fx.iter_mut())
                {
                    fx.reset();
                }
                self.drum_group_fx.reset();
                self.bus_fx.reset();
            }
            Command::SetSynth { param, value } => self.set_synth(param, value),
            Command::SetDrumMacro {
                model,
                param,
                value,
            } => self.set_drum_macro(model, param, value),
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
                self.sequencer
                    .launch(slot as usize, absolute_frame, quantize, &self.transport);
            }
            Command::StopClip { slot, quantize } => {
                self.sequencer
                    .stop(slot as usize, absolute_frame, quantize, &self.transport);
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
                    match ev.kind {
                        ClipEventKind::NoteOff { channel, note } => {
                            self.voices.note_off_recorded(channel, note);
                            self.fm.note_off_recorded(channel, note);
                        }
                        ClipEventKind::TouchUp { owner } => {
                            self.clip_touch_up(owner as u32, ev.slot as u8);
                        }
                        _ => {}
                    }
                }
            }
            Command::SetClipMode { slot, mode } => {
                if let Some(s) = self.sequencer.slot_mut(slot as usize) {
                    s.set_mode(mode);
                }
            }
            Command::SetClipGain { slot, value } => {
                let i = (slot as usize).min(MAX_CLIPS - 1);
                self.clip_gain[i] = value.clamp(0.0, 2.0);
            }
            Command::StartRepeat {
                owner,
                channel,
                note,
                velocity,
                division,
            } => {
                if channel == DRUM_CHANNEL {
                    self.drums.trigger(drum_model_for_note(note), velocity);
                } else {
                    let group = self.bank.nearer_index();
                    self.voices.note_on(channel, note, velocity, group);
                    self.voices.note_off(channel, note);
                }
                self.repeats.start(
                    owner,
                    channel,
                    note,
                    velocity,
                    division,
                    absolute_frame,
                    &self.transport,
                );
            }
            Command::StopRepeat { owner } => self.repeats.stop(owner),
            Command::StopAllRepeats => self.repeats.stop_all(),
            Command::RepeatHit {
                owner,
                channel,
                note,
                velocity,
            } => {
                if !self.repeats.contains(owner) {
                    return;
                }
                if channel == DRUM_CHANNEL {
                    self.drums.trigger(drum_model_for_note(note), velocity);
                } else {
                    let group = self.bank.nearer_index();
                    self.voices.note_on(channel, note, velocity, group);
                    self.voices.note_off(channel, note);
                }
            }
            Command::TouchDown {
                owner,
                x,
                y,
                channel,
                velocity,
            } => {
                let (xf, yf) = unpack_xy(x, y);
                // Tone / morph / vib are owned by the UI (Kaoss program Y). Do not
                // force Y→tone here — that made VIB/MORPH scrub brightness too.
                match self.kaoss.down(owner, xf, yf, channel, velocity) {
                    TouchDelta::Start {
                        channel,
                        note,
                        velocity,
                    } => self.start_touch_note(channel, note, velocity),
                    other => self.apply_touch_delta(other),
                }
            }
            Command::TouchUp { owner } => {
                let delta = self.kaoss.up(owner);
                self.apply_touch_delta(delta);
            }
            Command::TouchCancel { owner } => {
                let delta = self.kaoss.up(owner);
                self.apply_touch_delta(delta);
            }
            Command::SetKaossScale {
                scale_index,
                key,
                root_midi,
                octaves,
            } => {
                self.kaoss.configure(scale_index, key, root_midi, octaves);
                self.kaoss_seq.configure(scale_index, key, root_midi, octaves);
            }
            Command::ClipTouchDown {
                owner,
                x,
                y,
                slot,
                mode,
            } => self.clip_touch_down(owner, x, y, slot, mode),
            Command::ClipTouchMove { owner, x, y, slot } => self.clip_touch_move(owner, x, y, slot),
            Command::ClipTouchUp { owner, slot } => self.clip_touch_up(owner, slot),
            Command::SetEmitMode { target, mode } => {
                let mode = EmitMode::from_u8(mode);
                match target {
                    1 => self.kaoss_emit = mode,
                    2 => self.arp_emit = mode,
                    _ => self.clip_emit = mode,
                }
            }
            Command::SetArp {
                enabled,
                latch,
                order,
                division,
                octaves,
                gate,
            } => {
                self.arp.set_order(ArpOrder::from_u8(order));
                self.arp.set_division(ArpDivision::from_u8(division));
                self.arp.set_octaves(octaves);
                self.arp.set_gate(gate);
                if let Some((ch, n)) = self.arp.set_latch(latch) {
                    self.apply_arp_voice(ch, n, 0, false, relative_frame, midi_out);
                }
                if let Some((ch, n)) = self.arp.set_enabled(enabled) {
                    self.apply_arp_voice(ch, n, 0, false, relative_frame, midi_out);
                }
            }
            Command::SetArpStep { index, interval } => self.arp.set_step(index, interval),
            Command::SetArpLen { len } => self.arp.set_len(len),
            Command::ArpVoice {
                channel,
                note,
                velocity,
                on,
            } => self.apply_arp_voice(channel, note, velocity, on, relative_frame, midi_out),
            Command::MidiEmit { status, d1, d2 } => {
                if let Some(ev) = MidiEvent::parse(&[status, d1, d2]) {
                    midi_out.push(relative_frame, ev);
                }
            }
        }
    }

    fn apply_arp_voice(
        &mut self,
        channel: u8,
        note: u8,
        velocity: u8,
        on: bool,
        relative_frame: u32,
        midi_out: &mut MidiOutSink,
    ) {
        if on {
            if let Some((ch, n)) = self.arp_live.take() {
                self.release_arp_pitch(ch, n, relative_frame, midi_out);
            }
            self.arp_live = Some((channel, note));
            if self.arp_emit.includes_local() {
                self.sound_on(channel, note, velocity, MixSource::Live);
            }
            if self.arp_emit.includes_usb() {
                midi_out.push(
                    relative_frame,
                    MidiEvent::NoteOn {
                        channel,
                        note,
                        velocity,
                    },
                );
            }
        } else {
            if self.arp_live == Some((channel, note)) {
                self.arp_live = None;
            }
            self.release_arp_pitch(channel, note, relative_frame, midi_out);
        }
    }

    fn release_arp_pitch(
        &mut self,
        channel: u8,
        note: u8,
        relative_frame: u32,
        midi_out: &mut MidiOutSink,
    ) {
        if self.arp_emit.includes_local() && channel != DRUM_CHANNEL {
            self.voices.note_off(channel, note);
            self.fm.note_off(channel, note);
        }
        if self.arp_emit.includes_usb() {
            midi_out.push(
                relative_frame,
                MidiEvent::NoteOff {
                    channel,
                    note,
                    velocity: 0,
                },
            );
        }
    }

    fn sound_on(&mut self, channel: u8, note: u8, velocity: u8, mix: MixSource) {
        if velocity == 0 {
            self.voices.note_off(channel, note);
            self.fm.note_off(channel, note);
            return;
        }
        if channel == DRUM_CHANNEL {
            self.drums
                .trigger_mix(drum_model_for_note(note), velocity, mix);
        } else if self.fm_enabled {
            self.fm.note_on(channel, note, velocity);
        } else {
            let group = self.bank.nearer_index();
            self.voices
                .note_on_mix(channel, note, velocity, group, mix);
        }
    }

    fn start_touch_note(&mut self, channel: u8, note: u8, velocity: u8) {
        self.sound_on(channel, note, velocity, MixSource::Live);
    }

    fn apply_touch_delta(&mut self, delta: TouchDelta) {
        self.apply_touch_delta_mix(delta, MixSource::Live, false, false);
    }

    fn apply_touch_delta_mix(
        &mut self,
        delta: TouchDelta,
        mix: MixSource,
        recorded: bool,
        from_seq: bool,
    ) {
        match delta {
            TouchDelta::Idle => {}
            TouchDelta::Start {
                channel,
                note,
                velocity,
            } => self.sound_on(channel, note, velocity, mix),
            TouchDelta::Retune {
                channel,
                old_note,
                new_note,
                velocity,
            } => {
                let held = if from_seq {
                    self.kaoss_seq.note_is_held(channel, old_note)
                } else {
                    self.kaoss.note_is_held(channel, old_note)
                };
                if channel != DRUM_CHANNEL && !held {
                    if !self.voices.retune(channel, old_note, new_note, recorded) {
                        if recorded {
                            self.voices.note_off_recorded(channel, old_note);
                        } else {
                            self.voices.note_off(channel, old_note);
                        }
                        self.sound_on(channel, new_note, velocity, mix);
                    }
                } else {
                    self.sound_on(channel, new_note, velocity, mix);
                }
            }
            TouchDelta::Stop { channel, note } => {
                let held = if from_seq {
                    self.kaoss_seq.note_is_held(channel, note)
                } else {
                    self.kaoss.note_is_held(channel, note)
                };
                if channel != DRUM_CHANNEL && !held {
                    if recorded {
                        self.voices.note_off_recorded(channel, note);
                    } else {
                        self.voices.note_off(channel, note);
                    }
                }
            }
        }
    }

    fn clip_touch_down(&mut self, owner: u32, x: u16, y: u16, slot: u8, mode: u8) {
        let (xf, yf) = unpack_xy(x, y);
        let mix = MixSource::seq_kaoss(slot as usize);
        let vel = crate::kaoss::velocity_at_y(yf);
        let idx = (owner as usize) % crate::kaoss::MAX_TOUCH_VOICES;
        self.kaoss_seq_mode[idx] = mode;
        let delta = self.kaoss_seq.down(owner, xf, yf, 0, vel);
        self.apply_touch_delta_mix(delta, mix, true, true);
        self.apply_clip_touch_y(owner, yf);
    }

    fn clip_touch_move(&mut self, owner: u32, x: u16, y: u16, slot: u8) {
        let (xf, yf) = unpack_xy(x, y);
        let mix = MixSource::seq_kaoss(slot as usize);
        let vel = crate::kaoss::velocity_at_y(yf);
        let delta = self.kaoss_seq.follow(owner, xf, yf, vel);
        self.apply_touch_delta_mix(delta, mix, true, true);
        self.apply_clip_touch_y(owner, yf);
    }

    fn clip_touch_up(&mut self, owner: u32, _slot: u8) {
        let delta = self.kaoss_seq.up(owner);
        self.apply_touch_delta_mix(
            delta,
            MixSource::seq_kaoss(SEQ_KAOSS_MIX_SLOT as usize),
            true,
            true,
        );
    }

    fn apply_clip_touch_y(&mut self, owner: u32, y: f32) {
        let Some((channel, note)) = self.kaoss_seq.note_for_owner(owner) else {
            return;
        };
        let idx = (owner as usize) % crate::kaoss::MAX_TOUCH_VOICES;
        if self.kaoss_seq_mode[idx] == 1 {
            self.voices
                .set_voice_bend(channel, note, true, KaossMapper::y_to_bend_semis(y));
        } else {
            self.voices.set_voice_tone(channel, note, true, y);
        }
    }

    fn release_kaoss(&mut self) {
        for delta in self.kaoss.stop_all() {
            self.apply_touch_delta(delta);
        }
        let deltas = self.kaoss_seq.stop_all();
        for delta in deltas {
            self.apply_touch_delta_mix(
                delta,
                MixSource::seq_kaoss(SEQ_KAOSS_MIX_SLOT as usize),
                true,
                true,
            );
        }
    }

    /// Apply coalesced XY updates. Only active gestures move; a lift already
    /// processed in this block wins because `KaossMapper::follow` is idle then.
    ///
    /// Brightness / morph / vibrato come from UI `synth` commands for the active
    /// Kaoss program — this path only retunes pitch from X.
    pub fn sync_touches(&mut self, touches: &[LatestTouch]) {
        for touch in touches {
            let touch = touch.clamp();
            let delta = self
                .kaoss
                .follow(touch.owner, touch.x, touch.y, touch.velocity);
            self.apply_touch_delta(delta);
        }
    }

    fn set_synth(&mut self, param: SynthParam, value: f32) {
        let unit = value.clamp(0.0, 1.0);
        let mut macros = self.drums.macros();
        match param {
            SynthParam::Morph => self.bank.set_morph(unit),
            SynthParam::Tone => self.tone = unit,
            SynthParam::Level => self.level = unit,
            SynthParam::Attack => {
                self.attack_sec = map_exp_time(unit, ATTACK_SEC_MIN, ATTACK_SEC_MAX)
            }
            SynthParam::Release => {
                self.release_sec = map_exp_time(unit, RELEASE_SEC_MIN, RELEASE_SEC_MAX)
            }
            SynthParam::VibratoDepth => self.vib_depth_semis = unit * 2.0,
            SynthParam::VibratoRate => self.vib_rate_hz = 1.0 + unit * 8.0,
            SynthParam::VibratoMod => self.vib_mod = unit,
            SynthParam::VibratoAlways => self.vib_always = unit,
            SynthParam::ToneLfoRate => self.tone_lfo_rate_hz = tone_lfo_hz_from_unit(unit),
            SynthParam::ToneLfoAmount => self.tone_lfo_amount = unit,
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
            SynthParam::DrumLevel => self.drum_level = unit,
            SynthParam::FmEnable => {
                let enable = value > 0.5;
                if enable != self.fm_enabled {
                    if enable {
                        self.voices.all_notes_off_live();
                    } else {
                        self.fm.all_notes_off();
                    }
                    self.fm_enabled = enable;
                }
            }
            SynthParam::FmRecipe => {
                self.fm.set_recipe(value.round().clamp(0.0, 7.0) as usize);
            }
            SynthParam::FmOp => {
                self.fm.set_selected(value.round() as usize);
            }
            SynthParam::FmConnect => {
                let (from, to, amount) = crate::fm::unpack_fm_link(value);
                self.fm.set_link(from, to, amount);
            }
            SynthParam::FmClear => {
                self.fm.clear_links();
            }
            SynthParam::FmBright => self.fm.set_bright(unit),
            SynthParam::FmClang => self.fm.set_clang(unit),
            SynthParam::FmHit => self.fm.set_hit(unit),
            SynthParam::FmTail => self.fm.set_tail(unit),
        }
    }

    fn set_drum_macro(&mut self, model: u8, param: SynthParam, value: f32) {
        let unit = value.clamp(0.0, 1.0);
        let model = DrumModel::from_index(model as usize);
        let mut macros = self.drums.macros_for(model);
        match param {
            SynthParam::DrumPitch => macros.pitch = unit,
            SynthParam::DrumDecay => macros.decay = unit,
            SynthParam::DrumNoise => macros.noise = unit,
            SynthParam::DrumTone => macros.tone = unit,
            _ => return,
        }
        self.drums.set_model_macros(model, macros);
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
            FxParam::FlangerMix => params.flanger_mix = unit,
            FxParam::FlangerRate => params.flanger_rate = unit,
            FxParam::FlangerDepth => params.flanger_depth = unit,
            FxParam::FlangerFb => params.flanger_fb = unit,
        }
        slot.set_params(params);
    }

    /// Drum macros, for status / UI mirroring.
    pub fn drum_macros(&self) -> DrumMacros {
        self.drums.macros()
    }

    pub fn drum_macros_for(&self, model: DrumModel) -> DrumMacros {
        self.drums.macros_for(model)
    }

    /// Clip slot mode, for status / UI mirroring.
    pub fn clip_mode(&self, slot: usize) -> Option<LaunchMode> {
        self.sequencer.slot(slot).map(|s| s.mode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Clip, ClipEvent, ClipVoice, LaunchMode, SEQ_CLIP_SLOT};
    use crate::kaoss::LatestTouch;
    use crate::transport::{Quantize, PPQ};

    fn engine() -> JamboxEngine {
        let mut e = JamboxEngine::new(48_000.0);
        apply_now(&mut e, Command::SetTempo { bpm: 120.0 });
        e
    }

    fn apply_now(e: &mut JamboxEngine, command: Command) {
        let mut midi = MidiOutSink::new();
        e.apply(command, 0, 0, &mut midi);
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
    fn fm_enable_routes_keys_to_the_fm_playground() {
        let mut e = engine();
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::FmEnable,
                value: 1.0,
            },
        );
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::FmRecipe,
                value: 0.0,
            },
        );
        let mut out = vec![0.0f32; 1024];
        let mut midi = MidiOutSink::new();
        let cmds = [ScheduledCommand::now(Command::NoteOn {
            channel: 0,
            note: 72,
            velocity: 120,
        })];
        e.render(&mut out, &cmds, &mut midi);
        assert!(
            peak(&out) > 0.01,
            "FM playground should be audible, peak={}",
            peak(&out)
        );
        assert_eq!(e.voices.active_count(), 0, "wavetable should stay quiet");
        assert_eq!(e.fm.active_count(), 1);
        assert_eq!(e.status().active_voices, 1);
    }

    #[test]
    fn fm_connect_keeps_the_packed_draw_value() {
        let mut e = engine();
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::FmEnable,
                value: 1.0,
            },
        );
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::FmClear,
                value: 1.0,
            },
        );
        let packed = crate::fm::pack_fm_link(0, 3, 0.8);
        assert!(
            packed > 1.0,
            "packed draw must survive the 0..1 unit clamp, got {packed}"
        );
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::FmConnect,
                value: packed,
            },
        );
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::FmOp,
                value: 1.0,
            },
        );
        assert!((e.fm.patch().matrix[0][3] - 0.8).abs() < 0.02);
        assert_eq!(e.fm.selected(), 1);
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
    fn keys_level_does_not_duck_the_kit() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[
                ScheduledCommand::now(Command::SetSynth {
                    param: SynthParam::Level,
                    value: 0.0,
                }),
                ScheduledCommand::now(Command::NoteOn {
                    channel: DRUM_CHANNEL,
                    note: 36,
                    velocity: 120,
                }),
            ],
            &mut midi,
        );
        assert!(
            peak(&out) > 0.05,
            "kit must still speak when melody level is zero"
        );
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::SetSynth {
                param: SynthParam::DrumLevel,
                value: 0.0,
            })],
            &mut midi,
        );
        assert_eq!(peak(&out), 0.0, "drum_level 0 must silence the kit");
    }

    #[test]
    fn drum_level_does_not_duck_the_keys() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[
                ScheduledCommand::now(Command::SetSynth {
                    param: SynthParam::DrumLevel,
                    value: 0.0,
                }),
                ScheduledCommand::now(Command::NoteOn {
                    channel: 0,
                    note: 69,
                    velocity: 120,
                }),
            ],
            &mut midi,
        );
        assert!(
            peak(&out) > 0.01,
            "melody must still speak when kit level is zero"
        );
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::SetSynth {
                param: SynthParam::Level,
                value: 0.0,
            })],
            &mut midi,
        );
        assert_eq!(peak(&out), 0.0, "keys level 0 must silence the melody");
    }

    #[test]
    fn live_level_does_not_duck_clip_keys() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[
                ScheduledCommand::now(Command::SetSynth {
                    param: SynthParam::Level,
                    value: 0.0,
                }),
                ScheduledCommand::now(Command::ClipNoteOn {
                    slot: 0,
                    channel: 0,
                    note: 69,
                    velocity: 120,
                }),
            ],
            &mut midi,
        );
        assert!(
            peak(&out) > 0.01,
            "sequenced keys must still speak when LIVE is down"
        );
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::SetClipGain {
                slot: 0,
                value: 0.0,
            })],
            &mut midi,
        );
        assert_eq!(peak(&out), 0.0, "clip gain 0 must silence that clip");
    }

    #[test]
    fn clip_gain_does_not_duck_live_keys() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[
                ScheduledCommand::now(Command::SetClipGain {
                    slot: 0,
                    value: 0.0,
                }),
                ScheduledCommand::now(Command::NoteOn {
                    channel: 0,
                    note: 69,
                    velocity: 120,
                }),
            ],
            &mut midi,
        );
        assert!(
            peak(&out) > 0.01,
            "live keys must still speak when a clip fader is down"
        );
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
        apply_now(
            &mut e,
            Command::LaunchClip {
                slot: 0,
                quantize: Quantize::Off,
            },
        );

        let mut out = vec![0.0f32; 256];
        let mut midi = MidiOutSink::new();
        e.render(&mut out, &[], &mut midi);
        assert_eq!(midi.len(), 1);
        assert!(matches!(midi.as_slice()[0].1, MidiEvent::NoteOn { .. }));
    }

    #[test]
    fn clip_local_only_plays_locally_without_midi_out() {
        let mut e = engine();
        apply_now(
            &mut e,
            Command::SetEmitMode {
                target: 0,
                mode: EmitMode::Local as u8,
            },
        );
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
        apply_now(
            &mut e,
            Command::LaunchClip {
                slot: 0,
                quantize: Quantize::Off,
            },
        );
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        e.render(&mut out, &[], &mut midi);
        assert!(midi.is_empty());
        assert!(peak(&out) > 0.01);
        assert_eq!(e.status().active_voices, 1);
    }

    #[test]
    fn clip_usb_only_emits_midi_without_local_voices() {
        let mut e = engine();
        apply_now(
            &mut e,
            Command::SetEmitMode {
                target: 0,
                mode: EmitMode::Usb as u8,
            },
        );
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
        apply_now(
            &mut e,
            Command::LaunchClip {
                slot: 0,
                quantize: Quantize::Off,
            },
        );
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        e.render(&mut out, &[], &mut midi);
        assert_eq!(midi.len(), 1);
        assert!(matches!(midi.as_slice()[0].1, MidiEvent::NoteOn { .. }));
        assert_eq!(e.status().active_voices, 0);
        assert_eq!(peak(&out), 0.0);
    }

    #[test]
    fn midi_emit_appears_in_sink() {
        let mut e = engine();
        let mut out = vec![0.0f32; 64];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::MidiEmit {
                status: 0xb0,
                d1: 12,
                d2: 64,
            })],
            &mut midi,
        );
        assert_eq!(midi.len(), 1);
        assert!(matches!(
            midi.as_slice()[0].1,
            MidiEvent::ControlChange {
                controller: 12,
                value: 64,
                ..
            }
        ));
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
    fn voice_delay_keeps_echoing_after_the_note_ends() {
        let mut e = engine();
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::Release,
                value: 0.0,
            },
        );
        apply_now(
            &mut e,
            Command::SetFx {
                target: FxTarget::Voice(0),
                param: FxParam::DelayTime,
                value: 0.0,
            },
        );
        apply_now(
            &mut e,
            Command::SetFx {
                target: FxTarget::Voice(0),
                param: FxParam::DelayFb,
                value: 0.0,
            },
        );
        apply_now(
            &mut e,
            Command::SetFx {
                target: FxTarget::Voice(0),
                param: FxParam::DelayMix,
                value: 0.7,
            },
        );

        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::NoteOn {
                channel: 0,
                note: 69,
                velocity: 120,
            })],
            &mut midi,
        );
        assert!(peak(&out) > 0.01, "note should speak");

        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::NoteOff {
                channel: 0,
                note: 69,
            })],
            &mut midi,
        );
        for _ in 0..8 {
            if e.status().active_voices == 0 {
                break;
            }
            e.render(&mut out, &[], &mut midi);
        }
        assert_eq!(
            e.status().active_voices, 0,
            "dry voice must be gone before we listen for the echo"
        );

        // 50 ms delay; the note lived in the first ~20 ms, so the repeat
        // lands just after the voice is reclaimed.
        let mut tail = 0.0f32;
        for _ in 0..12 {
            e.render(&mut out, &[], &mut midi);
            tail = tail.max(peak(&out));
        }
        assert!(
            tail > 0.01,
            "insert delay should keep ringing after the strum lifts, peak={tail}"
        );
    }

    #[test]
    fn tone_lowpass_mid_is_brighter_than_dark() {
        let sr = 44100.0_f32;
        let src: Vec<f32> = (0..512)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let mut dark = src.clone();
        let mut mid = src.clone();
        let mut lp_d = 0.0_f32;
        let mut bp_d = 0.0_f32;
        let mut lp_m = 0.0_f32;
        let mut bp_m = 0.0_f32;
        apply_tone_lowpass(&mut dark, 0.0, &mut lp_d, &mut bp_d, sr);
        apply_tone_lowpass(&mut mid, 0.5, &mut lp_m, &mut bp_m, sr);
        let peak = |v: &[f32]| v.iter().fold(0.0_f32, |m, x| m.max(x.abs()));
        assert!(peak(&mid) > peak(&dark) * 1.2);
    }

    #[test]
    fn attack_knob_uses_python_exponential_map() {
        assert!((map_exp_time(0.0, ATTACK_SEC_MIN, ATTACK_SEC_MAX) - 0.002).abs() < 1e-6);
        assert!((map_exp_time(1.0, ATTACK_SEC_MIN, ATTACK_SEC_MAX) - 0.400).abs() < 1e-6);
        let mid = map_exp_time(0.5, ATTACK_SEC_MIN, ATTACK_SEC_MAX);
        // Linear map would be ~0.201 s; Python's log curve is ~0.028 s.
        assert!(mid < 0.04 && mid > 0.02);
    }

    #[test]
    fn vibrato_stays_off_until_mod_or_always() {
        let mut dry = engine();
        let mut wet = engine();
        for e in [&mut dry, &mut wet] {
            apply_now(
                e,
                Command::SetSynth {
                    param: SynthParam::VibratoDepth,
                    value: 1.0,
                },
            );
            apply_now(
                e,
                Command::NoteOn {
                    channel: 0,
                    note: 69,
                    velocity: 127,
                },
            );
        }
        apply_now(
            &mut wet,
            Command::SetSynth {
                param: SynthParam::VibratoAlways,
                value: 1.0,
            },
        );
        let mut dry_buf = vec![0.0f32; 2048];
        let mut wet_buf = vec![0.0f32; 2048];
        let mut midi = MidiOutSink::new();
        dry.render(&mut dry_buf, &[], &mut midi);
        wet.render(&mut wet_buf, &[], &mut midi);
        let diff: f32 = dry_buf
            .iter()
            .zip(wet_buf.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(
            diff > 0.5,
            "always-on vibrato should detune vs gated-off, diff={diff}"
        );
    }

    #[test]
    fn tone_lfo_hz_is_slow_at_the_bottom_and_fast_at_the_top() {
        assert!((tone_lfo_hz_from_unit(0.0) - TONE_LFO_HZ_MIN).abs() < 1e-5);
        assert!((tone_lfo_hz_from_unit(1.0) - TONE_LFO_HZ_MAX).abs() < 1e-5);
        let mid = tone_lfo_hz_from_unit(0.5);
        assert!(mid > TONE_LFO_HZ_MIN * 1.5);
        assert!(mid < (TONE_LFO_HZ_MIN + TONE_LFO_HZ_MAX) * 0.5);
    }

    #[test]
    fn tone_lfo_moves_the_filter_vs_sticky_tone() {
        let mut dry = engine();
        let mut wet = engine();
        for e in [&mut dry, &mut wet] {
            apply_now(
                e,
                Command::SetSynth {
                    param: SynthParam::Tone,
                    value: 0.5,
                },
            );
            apply_now(
                e,
                Command::NoteOn {
                    channel: 0,
                    note: 60,
                    velocity: 127,
                },
            );
        }
        apply_now(
            &mut wet,
            Command::SetSynth {
                param: SynthParam::ToneLfoRate,
                value: 1.0,
            },
        );
        apply_now(
            &mut wet,
            Command::SetSynth {
                param: SynthParam::ToneLfoAmount,
                value: 1.0,
            },
        );
        let mut dry_buf = vec![0.0f32; 2048];
        let mut wet_buf = vec![0.0f32; 2048];
        let mut midi = MidiOutSink::new();
        // Let envelopes match, then compare a block where the LFO has moved.
        dry.render(&mut dry_buf, &[], &mut midi);
        wet.render(&mut wet_buf, &[], &mut midi);
        dry.render(&mut dry_buf, &[], &mut midi);
        wet.render(&mut wet_buf, &[], &mut midi);
        let diff: f32 = dry_buf
            .iter()
            .zip(wet_buf.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(
            diff > 0.5,
            "tone LFO should wah the filter vs sticky tone, diff={diff}"
        );
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
        apply_now(
            &mut e,
            Command::LaunchClip {
                slot: 0,
                quantize: Quantize::Off,
            },
        );

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

    #[test]
    fn a_kaoss_touch_starts_and_releases_a_voice() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        let (x, y) = crate::kaoss::pack_xy(0.0, 1.0);
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::TouchDown {
                owner: 7,
                x,
                y,
                channel: 0,
                velocity: 120,
            })],
            &mut midi,
        );
        assert_eq!(e.status().active_voices, 1);
        assert!(peak(&out) > 0.01);
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::TouchUp { owner: 7 })],
            &mut midi,
        );
        assert_eq!(e.active_touches(), 0);
    }

    #[test]
    fn clip_kaoss_touch_sounds_on_the_kaoss_mix_bus() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        let (x, y) = crate::kaoss::pack_xy(0.2, 0.8);
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::ClipTouchDown {
                owner: 1,
                x,
                y,
                slot: SEQ_CLIP_SLOT,
                mode: 0,
            })],
            &mut midi,
        );
        assert!(
            peak(&out) > 0.01,
            "recorded Kaoss gesture should make sound"
        );
        apply_now(
            &mut e,
            Command::SetClipGain {
                slot: SEQ_KAOSS_MIX_SLOT,
                value: 0.0,
            },
        );
        e.render(&mut out, &[], &mut midi);
        assert!(
            peak(&out) < 0.02,
            "KSS fader at zero should mute the recorded pad"
        );
    }

    #[test]
    fn lifting_one_kaoss_finger_leaves_the_other_sounding() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        let (x_low, y) = crate::kaoss::pack_xy(0.0, 0.8);
        let (x_high, _) = crate::kaoss::pack_xy(1.0, 0.8);
        e.render(
            &mut out,
            &[
                ScheduledCommand::now(Command::TouchDown {
                    owner: 1,
                    x: x_low,
                    y,
                    channel: 0,
                    velocity: 120,
                }),
                ScheduledCommand::now(Command::TouchDown {
                    owner: 2,
                    x: x_high,
                    y,
                    channel: 0,
                    velocity: 120,
                }),
            ],
            &mut midi,
        );
        assert_eq!(e.active_touches(), 2);
        assert_eq!(e.voices.held_count(), 2);
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::TouchUp { owner: 1 })],
            &mut midi,
        );
        assert_eq!(e.active_touches(), 1);
        assert_eq!(e.voices.held_count(), 1);
        assert!(peak(&out) > 0.01);
    }

    #[test]
    fn smearing_one_kaoss_finger_onto_another_does_not_steal() {
        let mut e = engine();
        let mut out = vec![0.0f32; 512];
        let mut midi = MidiOutSink::new();
        let (x_low, y) = crate::kaoss::pack_xy(0.0, 0.8);
        let (x_high, _) = crate::kaoss::pack_xy(1.0, 0.8);
        e.render(
            &mut out,
            &[
                ScheduledCommand::now(Command::TouchDown {
                    owner: 1,
                    x: x_low,
                    y,
                    channel: 0,
                    velocity: 120,
                }),
                ScheduledCommand::now(Command::TouchDown {
                    owner: 2,
                    x: x_high,
                    y,
                    channel: 0,
                    velocity: 120,
                }),
            ],
            &mut midi,
        );
        e.sync_touches(&[LatestTouch {
            owner: 1,
            x: 1.0,
            y: 0.8,
            channel: 0,
            velocity: 120,
        }]);
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::TouchUp { owner: 1 })],
            &mut midi,
        );
        assert_eq!(e.active_touches(), 1);
        assert_eq!(
            e.voices.held_count(),
            1,
            "the remaining finger's note must keep sounding after a smear lift"
        );
        assert!(peak(&out) > 0.01);
    }

    #[test]
    fn coalesced_moves_cannot_keep_a_note_after_lift() {
        let mut e = engine();
        let mut out = vec![0.0f32; 256];
        let mut midi = MidiOutSink::new();
        let (x, y) = crate::kaoss::pack_xy(0.0, 0.8);
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::TouchDown {
                owner: 3,
                x,
                y,
                channel: 0,
                velocity: 110,
            })],
            &mut midi,
        );
        let moves: Vec<LatestTouch> = (0..100)
            .map(|i| LatestTouch {
                owner: 3,
                x: i as f32 / 99.0,
                y: 0.8,
                channel: 0,
                velocity: 110,
            })
            .collect();
        e.sync_touches(&moves);
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::TouchUp { owner: 3 })],
            &mut midi,
        );
        e.sync_touches(&moves[50..]);
        assert_eq!(e.active_touches(), 0);
    }

    #[test]
    fn a_held_repeat_keeps_time_while_other_pads_fire() {
        let mut e = engine();
        let mut out = vec![0.0f32; 128];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::StartRepeat {
                owner: 41,
                channel: DRUM_CHANNEL,
                note: 36,
                velocity: 110,
                division: crate::RepeatDivision::Quarter,
            })],
            &mut midi,
        );
        assert_eq!(e.status().active_repeats, 1);
        assert_eq!(e.status().active_drums, 1);

        let beat = e.transport().samples_per_beat() as u64;
        let mut pos = 128u64;
        let mut extra_hits = 0u32;
        while pos < beat {
            e.render(
                &mut out,
                &[ScheduledCommand::now(Command::NoteOn {
                    channel: DRUM_CHANNEL,
                    note: 38,
                    velocity: 120,
                })],
                &mut midi,
            );
            extra_hits += 1;
            pos += 128;
        }
        assert!(extra_hits > 0);
        assert_eq!(e.status().active_repeats, 1);
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::StopRepeat { owner: 41 })],
            &mut midi,
        );
        assert_eq!(e.status().active_repeats, 0);
    }

    #[test]
    fn drum_macro_command_is_per_model() {
        let mut e = engine();
        apply_now(
            &mut e,
            Command::SetDrumMacro {
                model: DrumModel::Kick.index() as u8,
                param: SynthParam::DrumPitch,
                value: 1.0,
            },
        );
        assert!((e.drum_macros_for(DrumModel::Kick).pitch - 1.0).abs() < 1e-6);
        assert!(
            (e.drum_macros_for(DrumModel::Snare).pitch - DrumMacros::default().pitch).abs() < 1e-6
        );
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::DrumPitch,
                value: 0.1,
            },
        );
        assert!((e.drum_macros_for(DrumModel::Kick).pitch - 0.1).abs() < 1e-6);
        assert!((e.drum_macros_for(DrumModel::Snare).pitch - 0.1).abs() < 1e-6);
    }

    fn energy(buf: &[f32]) -> f32 {
        buf.iter().map(|s| s * s).sum()
    }

    fn launch_held_clip(e: &mut JamboxEngine, note: u8, tone: f32) {
        let clip = Clip::new(
            vec![ClipEvent {
                tick: 0,
                kind: ClipEventKind::NoteOn {
                    channel: 0,
                    note,
                    velocity: 120,
                },
            }],
            PPQ * 8,
        );
        let slot = e.sequencer_mut().slot_mut(0).unwrap();
        slot.set_clip(Some(clip));
        slot.set_playback_tone(tone);
        apply_now(
            e,
            Command::LaunchClip {
                slot: 0,
                quantize: Quantize::Off,
            },
        );
    }

    #[test]
    fn live_tone_knob_does_not_rewrite_a_clip_loop() {
        let mut clip_only = engine();
        launch_held_clip(&mut clip_only, 72, 1.0);
        apply_now(
            &mut clip_only,
            Command::SetSynth {
                param: SynthParam::Tone,
                value: 1.0,
            },
        );
        let mut bright = vec![0.0f32; 2048];
        let mut midi = MidiOutSink::new();
        clip_only.render(&mut bright, &[], &mut midi);
        apply_now(
            &mut clip_only,
            Command::SetSynth {
                param: SynthParam::Tone,
                value: 0.0,
            },
        );
        let mut dark = vec![0.0f32; 2048];
        clip_only.render(&mut dark, &[], &mut midi);
        let bright_e = energy(&bright);
        let dark_e = energy(&dark);
        assert!(bright_e > 0.1, "clip should be audible, e={bright_e}");
        let ratio = dark_e / bright_e.max(1e-9);
        assert!(
            ratio > 0.7 && ratio < 1.4,
            "live tone must not darken a pad/SEQ loop, ratio={ratio}"
        );
    }

    #[test]
    fn live_melody_still_follows_the_tone_knob() {
        let mut bright = engine();
        let mut dark = engine();
        for (e, tone) in [(&mut bright, 1.0), (&mut dark, 0.0)] {
            apply_now(
                e,
                Command::SetSynth {
                    param: SynthParam::Tone,
                    value: tone,
                },
            );
            apply_now(
                e,
                Command::NoteOn {
                    channel: 0,
                    note: 72,
                    velocity: 127,
                },
            );
        }
        let mut bright_buf = vec![0.0f32; 2048];
        let mut dark_buf = vec![0.0f32; 2048];
        let mut midi = MidiOutSink::new();
        bright.render(&mut bright_buf, &[], &mut midi);
        dark.render(&mut dark_buf, &[], &mut midi);
        let bright_e = energy(&bright_buf);
        let dark_e = energy(&dark_buf);
        assert!(
            dark_e < bright_e * 0.55,
            "live melody should darken with tone, bright={bright_e} dark={dark_e}"
        );
    }

    #[test]
    fn clip_note_off_does_not_kill_a_live_melody_on_the_same_key() {
        let mut e = engine();
        launch_held_clip(&mut e, 60, 1.0);
        apply_now(
            &mut e,
            Command::NoteOn {
                channel: 0,
                note: 60,
                velocity: 127,
            },
        );
        let mut midi = MidiOutSink::new();
        let mut out = vec![0.0f32; 512];
        e.render(&mut out, &[], &mut midi);
        assert!(e.status().active_voices >= 2);

        apply_now(&mut e, Command::ClipNoteOff { channel: 0, note: 60 });
        for _ in 0..64 {
            out.iter_mut().for_each(|s| *s = 0.0);
            e.render(&mut out, &[], &mut midi);
        }
        assert_eq!(
            e.status().active_voices, 1,
            "live melody should keep sounding after the pad note ends"
        );
        assert!(peak(&out) > 0.01);
    }

    #[test]
    fn baked_clip_tone_stays_dark_when_live_tone_opens() {
        let mut e = engine();
        launch_held_clip(&mut e, 84, 0.0);
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::Tone,
                value: 1.0,
            },
        );
        let mut baked_dark = vec![0.0f32; 2048];
        let mut midi = MidiOutSink::new();
        e.render(&mut baked_dark, &[], &mut midi);

        let mut open = engine();
        launch_held_clip(&mut open, 84, 1.0);
        apply_now(
            &mut open,
            Command::SetSynth {
                param: SynthParam::Tone,
                value: 1.0,
            },
        );
        let mut baked_open = vec![0.0f32; 2048];
        open.render(&mut baked_open, &[], &mut midi);
        assert!(
            energy(&baked_dark) < energy(&baked_open) * 0.55,
            "a pad written dark should stay darker than an open take"
        );
    }

    fn load_locked_clip(e: &mut JamboxEngine, voice: ClipVoice) {
        let clip = Clip::new(
            vec![ClipEvent {
                tick: 0,
                kind: ClipEventKind::NoteOn {
                    channel: 0,
                    note: 72,
                    velocity: 120,
                },
            }],
            PPQ * 8,
        );
        let _ = e.apply_clip_update(0, Some(Box::new(clip)), Some(LaunchMode::Loop), None, Some(voice));
        apply_now(
            e,
            Command::LaunchClip {
                slot: 0,
                quantize: Quantize::Off,
            },
        );
    }

    fn correlate(a: &[f32], b: &[f32]) -> f32 {
        let n = a.len().min(b.len());
        let mut acc = 0.0;
        let mut na = 0.0;
        let mut nb = 0.0;
        for i in 0..n {
            acc += a[i] * b[i];
            na += a[i] * a[i];
            nb += b[i] * b[i];
        }
        acc / (na.sqrt() * nb.sqrt()).max(1e-9)
    }

    #[test]
    fn locked_clip_keeps_baked_morph_when_live_morph_moves() {
        let saw_voice = ClipVoice {
            locked: true,
            morph_a: 2,
            morph_b: 2,
            morph: 0.0,
            tone: 1.0,
            ..ClipVoice::default()
        };

        let mut locked = engine();
        apply_now(
            &mut locked,
            Command::SetMorphPair { a: 0, b: 0 },
        );
        apply_now(
            &mut locked,
            Command::SetSynth {
                param: SynthParam::Morph,
                value: 0.0,
            },
        );
        load_locked_clip(&mut locked, saw_voice);
        let mut locked_buf = vec![0.0f32; 2048];
        let mut midi = MidiOutSink::new();
        locked.render(&mut locked_buf, &[], &mut midi);

        let mut sine = engine();
        apply_now(&mut sine, Command::SetMorphPair { a: 0, b: 0 });
        apply_now(
            &mut sine,
            Command::SetSynth {
                param: SynthParam::Morph,
                value: 0.0,
            },
        );
        launch_held_clip(&mut sine, 72, 1.0);
        let mut sine_buf = vec![0.0f32; 2048];
        sine.render(&mut sine_buf, &[], &mut midi);

        let mut saw = engine();
        apply_now(&mut saw, Command::SetMorphPair { a: 2, b: 2 });
        apply_now(
            &mut saw,
            Command::SetSynth {
                param: SynthParam::Morph,
                value: 0.0,
            },
        );
        launch_held_clip(&mut saw, 72, 1.0);
        let mut saw_buf = vec![0.0f32; 2048];
        saw.render(&mut saw_buf, &[], &mut midi);

        assert!(
            correlate(&locked_buf, &saw_buf) > correlate(&locked_buf, &sine_buf),
            "locked pad should keep the saw snapshot, not follow live sine"
        );
    }

    #[test]
    fn locked_clip_fx_ignores_live_voice_fx() {
        let mut e = engine();
        let voice = ClipVoice {
            locked: true,
            morph_a: 0,
            morph_b: 0,
            delay_mix: 0.8,
            ..ClipVoice::default()
        };
        load_locked_clip(&mut e, voice);
        apply_now(
            &mut e,
            Command::SetFx {
                target: FxTarget::Voice(0),
                param: FxParam::DelayMix,
                value: 0.0,
            },
        );
        assert!(
            (e.clip_fx[0].params().delay_mix - 0.8).abs() < 1e-6,
            "live voice FX must not rewrite a locked pad insert"
        );
        assert!(e.voice_fx[0].params().delay_mix < 0.01);
    }

    #[test]
    fn fm_enable_does_not_reroute_or_kill_a_clip() {
        let mut e = engine();
        launch_held_clip(&mut e, 72, 1.0);
        let mut out = vec![0.0f32; 1024];
        let mut midi = MidiOutSink::new();
        e.render(&mut out, &[], &mut midi);
        assert!(e.voices.active_count() >= 1);
        apply_now(
            &mut e,
            Command::SetSynth {
                param: SynthParam::FmEnable,
                value: 1.0,
            },
        );
        out.iter_mut().for_each(|s| *s = 0.0);
        e.render(&mut out, &[], &mut midi);
        assert!(
            e.voices.active_count() >= 1,
            "entering FM must not release a playing pad"
        );
        assert_eq!(e.fm.active_count(), 0, "pad notes must not jump to FM");
        assert!(peak(&out) > 0.01);
    }

    #[test]
    fn arp_swallows_the_held_root_and_emits_the_pattern() {
        let mut e = engine();
        apply_now(
            &mut e,
            Command::SetArp {
                enabled: true,
                latch: true,
                order: crate::ArpOrder::Order.as_u8(),
                division: crate::ArpDivision::Quarter.as_u8(),
                octaves: 0,
                gate: 64,
            },
        );
        let mut out = vec![0.0f32; 512];
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
        assert!(
            e.status().arp_enabled,
            "arp should report enabled after a root"
        );
        assert!(
            e.status().active_voices >= 1,
            "first arp step should sound immediately"
        );
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::NoteOff {
                channel: 0,
                note: 60,
            })],
            &mut midi,
        );
        assert!(
            e.status().arp_latched,
            "release must keep a latched arp running"
        );
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::NoteOn {
                channel: 0,
                note: 62,
                velocity: 120,
            })],
            &mut midi,
        );
        assert_eq!(e.status().arp_root, 62);
    }

    #[test]
    fn arp_does_not_thru_the_root_as_a_block_chord() {
        let mut e = engine();
        apply_now(
            &mut e,
            Command::SetArp {
                enabled: true,
                latch: false,
                order: crate::ArpOrder::Up.as_u8(),
                division: crate::ArpDivision::Quarter.as_u8(),
                octaves: 0,
                gate: 40,
            },
        );
        let mut out = vec![0.0f32; 256];
        let mut midi = MidiOutSink::new();
        e.render(
            &mut out,
            &[
                ScheduledCommand::now(Command::NoteOn {
                    channel: 0,
                    note: 60,
                    velocity: 120,
                }),
                ScheduledCommand::now(Command::NoteOn {
                    channel: 0,
                    note: 64,
                    velocity: 120,
                }),
                ScheduledCommand::now(Command::NoteOn {
                    channel: 0,
                    note: 67,
                    velocity: 120,
                }),
            ],
            &mut midi,
        );
        assert!(
            e.status().active_voices <= 1,
            "authored arp is monophonic — roots must not stack as a chord"
        );
    }

    #[test]
    fn arp_unlatched_release_turns_the_voice_off() {
        let mut e = engine();
        apply_now(
            &mut e,
            Command::SetArp {
                enabled: true,
                latch: false,
                order: crate::ArpOrder::Up.as_u8(),
                division: crate::ArpDivision::Quarter.as_u8(),
                octaves: 0,
                gate: 90,
            },
        );
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
        assert!(e.voices.held_count() >= 1, "root should start the arp");
        e.render(
            &mut out,
            &[ScheduledCommand::now(Command::NoteOff {
                channel: 0,
                note: 60,
            })],
            &mut midi,
        );
        assert_eq!(
            e.voices.held_count(),
            0,
            "unlatched lift must release the sounding arp step"
        );
        assert!(!e.status().arp_latched);
    }
}
