//! Line-delimited JSON client with a reliable-edge queue and latest-XY mailbox.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use jambox_protocol::{
    MidiNotice, RepeatDivision, RepeatPhase, Request, Response, StatusReply, TouchPhase,
    WireClipEvent, PROTOCOL_VERSION,
};
use tracing::{info, warn};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

enum Stream {
    #[cfg(unix)]
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Stream {
    fn try_clone(&self) -> std::io::Result<Stream> {
        match self {
            #[cfg(unix)]
            Self::Unix(s) => Ok(Self::Unix(s.try_clone()?)),
            Self::Tcp(s) => Ok(Self::Tcp(s.try_clone()?)),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(unix)]
            Self::Unix(s) => s.write(buf),
            Self::Tcp(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(s) => s.flush(),
            Self::Tcp(s) => s.flush(),
        }
    }
}

impl std::io::Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(unix)]
            Self::Unix(s) => s.read(buf),
            Self::Tcp(s) => s.read(buf),
        }
    }
}

/// Outgoing control traffic. Edges are FIFO; moves overwrite by gesture.
#[derive(Default)]
pub struct Outbox {
    reliable: VecDeque<Request>,
    moves: Vec<(u32, Request)>,
}

impl Outbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn touch(&mut self, gesture: u32, phase: TouchPhase, x: f32, y: f32, channel: u8) {
        let request = Request::Touch {
            gesture,
            phase,
            x,
            y,
            channel: channel & 0x0f,
            velocity: 110,
        };
        match phase {
            TouchPhase::Move => self.push_move(gesture, request),
            TouchPhase::Up | TouchPhase::Cancel => {
                self.moves.retain(|(g, _)| *g != gesture);
                self.reliable.push_back(request);
            }
            TouchPhase::Down => self.reliable.push_back(request),
        }
    }

    pub fn repeat(
        &mut self,
        gesture: u32,
        phase: RepeatPhase,
        note: u8,
        channel: u8,
        velocity: u8,
        division: RepeatDivision,
    ) {
        self.reliable.push_back(Request::Repeat {
            gesture,
            phase,
            note,
            channel,
            velocity,
            division,
        });
    }

    pub fn note_on(&mut self, channel: u8, note: u8, velocity: u8) {
        self.reliable.push_back(Request::NoteOn {
            channel,
            note,
            velocity,
        });
    }

    pub fn note_off(&mut self, channel: u8, note: u8) {
        self.reliable.push_back(Request::NoteOff { channel, note });
    }

    pub fn synth(&mut self, param: &str, value: f32) {
        self.synth_drum(param, value, None);
    }

    pub fn synth_drum(&mut self, param: &str, value: f32, drum: Option<u8>) {
        self.reliable.push_back(Request::Synth {
            param: param.to_string(),
            value,
            drum,
        });
    }

    pub fn knob_map(&mut self, mode: &str) {
        self.reliable.push_back(Request::KnobMap {
            mode: mode.to_string(),
            fx_kind: None,
            fx_index: 0,
        });
    }

    pub fn morph_pair(&mut self, a: u16, b: u16) {
        self.reliable.push_back(Request::MorphPair { a, b });
    }

    pub fn clip_load(
        &mut self,
        slot: u8,
        length_ticks: u32,
        mode: &str,
        events: Vec<WireClipEvent>,
    ) {
        self.reliable.push_back(Request::ClipLoad {
            slot,
            length_ticks,
            mode: Some(mode.to_string()),
            events,
        });
    }

    pub fn clip_clear(&mut self, slot: u8) {
        self.reliable.push_back(Request::ClipClear { slot });
    }

    pub fn clip_launch(&mut self, slot: u8, quantize: &str) {
        self.reliable.push_back(Request::ClipLaunch {
            slot,
            quantize: Some(quantize.to_string()),
        });
    }

    pub fn clip_stop(&mut self, slot: u8, quantize: &str) {
        self.reliable.push_back(Request::ClipStop {
            slot,
            quantize: Some(quantize.to_string()),
        });
    }

    pub fn stop_all_clips(&mut self) {
        self.reliable.push_back(Request::StopAllClips);
    }

    pub fn tempo(&mut self, bpm: f32) {
        self.reliable.push_back(Request::Tempo { bpm });
    }

    pub fn panic(&mut self) {
        self.reliable.push_back(Request::Panic);
    }

    pub fn all_notes_off(&mut self) {
        self.reliable.push_back(Request::AllNotesOff);
    }

    pub fn fx_bus(&mut self, param: &str, value: f32) {
        self.reliable.push_back(Request::Fx {
            target: jambox_protocol::FxTargetSpec::Bus,
            param: param.to_string(),
            value,
        });
    }

    pub fn fx_voice(&mut self, index: u16, param: &str, value: f32) {
        self.reliable.push_back(Request::Fx {
            target: jambox_protocol::FxTargetSpec::Voice { index },
            param: param.to_string(),
            value,
        });
    }

    pub fn fx_drum_group(&mut self, param: &str, value: f32) {
        self.reliable.push_back(Request::Fx {
            target: jambox_protocol::FxTargetSpec::DrumGroup,
            param: param.to_string(),
            value,
        });
    }

    pub fn kaoss_scale(&mut self, scale_index: u8, key: u8, root_midi: u8, octaves: u8) {
        self.reliable.push_back(Request::KaossScale {
            scale_index,
            key,
            root_midi,
            octaves,
        });
    }

    pub fn midi_emit(
        &mut self,
        kind: &str,
        channel: u8,
        note: Option<u8>,
        velocity: Option<u8>,
        control: Option<u8>,
        value: Option<u16>,
    ) {
        self.reliable.push_back(Request::MidiEmit {
            kind: kind.to_string(),
            channel: channel & 0x0f,
            note,
            velocity,
            control,
            value,
        });
    }

    pub fn emit_mode(&mut self, target: &str, mode: &str) {
        self.reliable.push_back(Request::EmitMode {
            target: target.to_string(),
            mode: mode.to_string(),
        });
    }

    pub fn status(&mut self) {
        self.reliable.push_back(Request::Status);
    }

    pub fn hello(&mut self) {
        self.reliable.push_front(Request::Hello {
            protocol: PROTOCOL_VERSION,
            client: "pidi-native".into(),
            realtime_owner: true,
        });
    }

    fn push_move(&mut self, gesture: u32, request: Request) {
        if let Some(existing) = self.moves.iter_mut().find(|(g, _)| *g == gesture) {
            existing.1 = request;
        } else {
            self.moves.push((gesture, request));
        }
    }

    /// Reliable edges first, then at most one move per gesture.
    pub fn take(&mut self) -> Vec<Request> {
        let mut out = Vec::with_capacity(self.reliable.len() + self.moves.len());
        while let Some(req) = self.reliable.pop_front() {
            out.push(req);
        }
        for (_, req) in self.moves.drain(..) {
            out.push(req);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.reliable.is_empty() && self.moves.is_empty()
    }
}

pub struct NativeClient {
    stream: Option<Stream>,
    reader: Option<BufReader<Stream>>,
    pub outbox: Outbox,
    pub last_status: StatusReply,
    pub midi_inbox: Vec<MidiNotice>,
    pub connected: bool,
    address: String,
    tcp: bool,
}

impl NativeClient {
    pub fn new(address: String, tcp: bool) -> Self {
        let mut client = Self {
            stream: None,
            reader: None,
            outbox: Outbox::new(),
            last_status: StatusReply::default(),
            midi_inbox: Vec::new(),
            connected: false,
            address,
            tcp,
        };
        client.reconnect();
        client
    }

    pub fn reconnect(&mut self) {
        self.connected = false;
        self.stream = None;
        self.reader = None;
        let opened = if self.tcp {
            TcpStream::connect(&self.address).ok().map(|s| {
                let _ = s.set_nodelay(true);
                let _ = s.set_read_timeout(Some(Duration::from_millis(1)));
                Stream::Tcp(s)
            })
        } else {
            #[cfg(unix)]
            {
                UnixStream::connect(&self.address).ok().map(|s| {
                    let _ = s.set_read_timeout(Some(Duration::from_millis(1)));
                    Stream::Unix(s)
                })
            }
            #[cfg(not(unix))]
            {
                None
            }
        };
        let Some(stream) = opened else {
            return;
        };
        let reader = match stream.try_clone() {
            Ok(clone) => BufReader::new(clone),
            Err(err) => {
                warn!(%err, "native: clone failed");
                return;
            }
        };
        self.stream = Some(stream);
        self.reader = Some(reader);
        self.connected = true;
        self.outbox.hello();
        info!(addr = %self.address, "native: connected");
    }

    pub fn flush(&mut self) {
        if !self.connected {
            self.reconnect();
            if !self.connected {
                self.outbox.take();
                return;
            }
        }
        let batch = self.outbox.take();
        for request in batch {
            if !self.write_request(&request) {
                self.connected = false;
                return;
            }
        }
        self.drain_replies();
    }

    fn write_request(&mut self, request: &Request) -> bool {
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };
        let Ok(json) = serde_json::to_string(request) else {
            return true;
        };
        writeln!(stream, "{json}").is_ok() && stream.flush().is_ok()
    }

    fn drain_replies(&mut self) {
        let Some(reader) = self.reader.as_mut() else {
            return;
        };
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    self.connected = false;
                    break;
                }
                Ok(_) => {
                    if let Ok(response) = serde_json::from_str::<Response>(line.trim()) {
                        match response {
                            Response::Status(status) => self.last_status = status,
                            Response::Midi(notice) => self.midi_inbox.push(notice),
                            _ => {}
                        }
                    }
                }
                Err(err)
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => {
                    self.connected = false;
                    break;
                }
            }
            if !reader.buffer().contains(&b'\n') {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_overwrite_and_edges_stay_in_order() {
        let mut box_ = Outbox::new();
        box_.touch(1, TouchPhase::Down, 0.1, 0.2, 0);
        box_.touch(1, TouchPhase::Move, 0.2, 0.2, 0);
        box_.touch(1, TouchPhase::Move, 0.9, 0.8, 0);
        box_.touch(1, TouchPhase::Up, 0.9, 0.8, 0);
        let batch = box_.take();
        assert_eq!(batch.len(), 2);
        assert!(matches!(
            batch[0],
            Request::Touch {
                phase: TouchPhase::Down,
                ..
            }
        ));
        assert!(matches!(
            batch[1],
            Request::Touch {
                phase: TouchPhase::Up,
                ..
            }
        ));
    }

    #[test]
    fn hello_is_sent_ahead_of_notes() {
        let mut box_ = Outbox::new();
        box_.note_on(0, 60, 100);
        box_.hello();
        let batch = box_.take();
        assert!(matches!(batch[0], Request::Hello { .. }));
        assert!(matches!(batch[1], Request::NoteOn { note: 60, .. }));
    }

    #[test]
    fn midi_emit_encodes_cc_and_notes() {
        let mut box_ = Outbox::new();
        box_.midi_emit("cc", 0, None, None, Some(12), Some(64));
        box_.midi_emit("note_on", 2, Some(60), Some(100), None, None);
        box_.emit_mode("clips", "usb");
        let batch = box_.take();
        assert!(matches!(
            &batch[0],
            Request::MidiEmit {
                kind,
                control: Some(12),
                value: Some(64),
                ..
            } if kind == "cc"
        ));
        assert!(matches!(
            &batch[1],
            Request::MidiEmit {
                kind,
                channel: 2,
                note: Some(60),
                velocity: Some(100),
                ..
            } if kind == "note_on"
        ));
        assert!(matches!(
            &batch[2],
            Request::EmitMode { target, mode }
                if target == "clips" && mode == "usb"
        ));
    }
}
