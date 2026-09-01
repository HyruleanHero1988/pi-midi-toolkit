//! Control server: line-delimited JSON over a Unix socket (or TCP on other hosts).
//!
//! This thread owns all parsing and allocation. It can block, log, and touch the
//! filesystem freely — none of that reaches the audio callback.
//!
//! Reliable edges (down/up/cancel/panic) go on the command ring immediately.
//! Touch *moves* overwrite a latest-value mailbox so a lift never waits behind
//! a trail of obsolete XY samples.

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use jambox_core::{pack_xy, Command, LatestTouch};
use tracing::{debug, info, warn};

use crate::audio::AudioHealth;
use crate::bus::{ClipUpdate, ControlSide, MidiInSide};
use crate::midi::{ingest, MidiMap};
use crate::protocol::{decode, Decoded, MidiNotice, Request, Response, StatusReply};
use jambox_protocol::{
    HelloReply, RepeatPhase, TouchPhase, NATIVE_FEATURES, PROTOCOL_VERSION,
};
use midi_core::MidiEvent;
use std::sync::mpsc::{self, Sender};

type Shared = Arc<Mutex<(ControlSide, StatusCache)>>;

static NEXT_SESSION: AtomicU32 = AtomicU32::new(1);

/// Fan-out for unsolicited MIDI notices (engine → every connected UI).
#[derive(Default)]
pub struct ClientHub {
    txs: Mutex<Vec<Sender<String>>>,
}

impl ClientHub {
    pub fn subscribe(&self) -> (Sender<String>, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel();
        self.txs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(tx.clone());
        (tx, rx)
    }

    pub fn broadcast_midi(&self, event: MidiEvent) {
        let notice = MidiNotice::from_event(event);
        let line = match serde_json::to_string(&Response::Midi(notice)) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut guard = self.txs.lock().unwrap_or_else(|p| p.into_inner());
        guard.retain(|tx| tx.send(line.clone()).is_ok());
    }
}

/// Where the UI connects.
pub enum Endpoint {
    #[cfg(unix)]
    Unix(std::path::PathBuf),
    Tcp(String),
}

impl Endpoint {
    pub fn describe(&self) -> String {
        match self {
            #[cfg(unix)]
            Self::Unix(path) => path.display().to_string(),
            Self::Tcp(addr) => addr.clone(),
        }
    }
}

/// Shared status the control thread keeps fresh for `status` requests.
#[derive(Default)]
pub struct StatusCache {
    last: Option<crate::bus::StatusPacket>,
    dropped_commands: u64,
    emergency_releases: u64,
}

impl StatusCache {
    fn reply(&self, touch_overwrites: u64) -> StatusReply {
        let packet = self.last.unwrap_or_default();
        let s = packet.engine;
        StatusReply {
            position: s.position,
            bpm: s.bpm,
            active_voices: s.active_voices,
            active_drums: s.active_drums,
            active_repeats: s.active_repeats,
            playing_clips: s.playing_clips,
            peak: s.peak,
            callback_frames: packet.callback_frames,
            callback_micros: packet.callback_micros,
            callback_peak_micros: packet.callback_peak_micros,
            xruns: packet.xruns,
            command_drops: self.dropped_commands,
            emergency_releases: self.emergency_releases,
            touch_overwrites,
        }
    }
}

/// Per-connection gesture ownership. Disconnect cancels only this session.
#[derive(Debug)]
pub struct ClientSession {
    pub id: u32,
    pub hello: bool,
    pub realtime_owner: bool,
    pub client: String,
    gestures: Vec<u32>,
    repeats: Vec<u32>,
}

impl ClientSession {
    pub fn new() -> Self {
        Self {
            id: NEXT_SESSION.fetch_add(1, Ordering::Relaxed),
            hello: false,
            realtime_owner: false,
            client: String::new(),
            gestures: Vec::new(),
            repeats: Vec::new(),
        }
    }

    fn track_gesture(&mut self, gesture: u32) {
        if !self.gestures.contains(&gesture) {
            self.gestures.push(gesture);
        }
    }

    fn untrack_gesture(&mut self, gesture: u32) {
        self.gestures.retain(|g| *g != gesture);
    }

    fn track_repeat(&mut self, gesture: u32) {
        if !self.repeats.contains(&gesture) {
            self.repeats.push(gesture);
        }
    }

    fn untrack_repeat(&mut self, gesture: u32) {
        self.repeats.retain(|g| *g != gesture);
    }
}

impl Default for ClientSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle one client connection until it disconnects.
fn serve_client<R: BufRead>(
    reader: R,
    tx: Sender<String>,
    shared: &Shared,
    running: &AtomicBool,
    hub: &ClientHub,
    map: &MidiMap,
    midi_in: &Mutex<MidiInSide>,
    health: &AudioHealth,
) {
    let mut session = ClientSession::new();
    for line in reader.lines() {
        if !running.load(Ordering::Relaxed) {
            break;
        }
        let line = match line {
            Ok(l) => l,
            Err(err) => {
                debug!(%err, "client read ended");
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = {
            let mut guard = match shared.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let (control, cache) = &mut *guard;
            handle_line(
                trimmed,
                control,
                cache,
                hub,
                map,
                midi_in,
                health,
                &mut session,
            )
        };
        if let Ok(json) = serde_json::to_string(&response) {
            if tx.send(json).is_err() {
                break;
            }
        }
    }

    let mut guard = match shared.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let (control, cache) = &mut *guard;
    emergency_release(control, cache, &mut session);
}

fn send_or_drop(control: &mut ControlSide, cache: &mut StatusCache, command: Command) -> bool {
    if control.send(command) {
        true
    } else {
        cache.dropped_commands += 1;
        false
    }
}

/// Cancel every gesture this UI owned. MPK notes are left alone.
pub fn emergency_release(
    control: &mut ControlSide,
    cache: &mut StatusCache,
    session: &mut ClientSession,
) {
    if session.gestures.is_empty() && session.repeats.is_empty() {
        return;
    }
    cache.emergency_releases += 1;
    info!(
        session = session.id,
        client = %session.client,
        gestures = session.gestures.len(),
        repeats = session.repeats.len(),
        "control: emergency release on disconnect"
    );
    for owner in session.repeats.drain(..) {
        control.latest.clear(owner);
        send_or_drop(control, cache, Command::StopRepeat { owner });
    }
    for owner in session.gestures.drain(..) {
        control.latest.clear(owner);
        send_or_drop(control, cache, Command::TouchCancel { owner });
    }
    control.latest.clear_all();
}

/// Parse + dispatch one request. Kept separate so it is directly testable.
pub fn handle_line(
    line: &str,
    control: &mut ControlSide,
    cache: &mut StatusCache,
    hub: &ClientHub,
    map: &MidiMap,
    midi_in: &Mutex<MidiInSide>,
    health: &AudioHealth,
    session: &mut ClientSession,
) -> Response {
    control.collect_garbage();
    if let Some(status) = control.latest_status() {
        cache.last = Some(status);
    }

    let request: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(err) => {
            return Response::Error {
                message: format!("bad request: {err}"),
            }
        }
    };

    match decode(request) {
        Err(message) => Response::Error { message },
        Ok(Decoded::Hello {
            protocol,
            client,
            realtime_owner,
        }) => {
            session.hello = true;
            session.realtime_owner = realtime_owner;
            session.client = client;
            if protocol != PROTOCOL_VERSION {
                warn!(
                    got = protocol,
                    want = PROTOCOL_VERSION,
                    "control: protocol mismatch; continuing with engine version"
                );
            }
            Response::Hello(HelloReply {
                protocol: PROTOCOL_VERSION,
                engine: "jambox-engine".into(),
                features: NATIVE_FEATURES.iter().map(|s| (*s).to_string()).collect(),
            })
        }
        Ok(Decoded::StatusRequest) => Response::Status(cache.reply(control.latest.overwrites())),
        Ok(Decoded::KnobMap {
            mode,
            fx_kind,
            fx_index,
        }) => {
            map.apply_knob_map(&mode, fx_kind.as_deref(), fx_index);
            Response::Ok
        }
        Ok(Decoded::MidiIn(event)) => {
            let mut side = midi_in.lock().unwrap_or_else(|p| p.into_inner());
            ingest(event, hub, map, |c| side.send(c));
            Response::Ok
        }
        Ok(Decoded::Command(command)) => {
            if send_or_drop(control, cache, command) {
                Response::Ok
            } else {
                Response::Error {
                    message: "command ring full".to_string(),
                }
            }
        }
        Ok(Decoded::ClipUpdate {
            slot,
            clip,
            mode,
            tone,
        }) => {
            if control.send_clip(ClipUpdate {
                slot,
                clip,
                mode,
                tone,
            }) {
                Response::Ok
            } else {
                cache.dropped_commands += 1;
                Response::Error {
                    message: "clip ring full".to_string(),
                }
            }
        }
        Ok(Decoded::Touch {
            gesture,
            phase,
            x,
            y,
            channel,
            velocity,
        }) => handle_touch(
            control, cache, session, gesture, phase, x, y, channel, velocity,
        ),
        Ok(Decoded::Repeat {
            gesture,
            phase,
            note,
            channel,
            velocity,
            division,
        }) => handle_repeat(
            control, cache, session, gesture, phase, note, channel, velocity, division,
        ),
        Ok(Decoded::AudioReopen) => {
            info!("control: audio reopen requested");
            health.request_reopen();
            Response::Ok
        }
    }
}

fn handle_touch(
    control: &mut ControlSide,
    cache: &mut StatusCache,
    session: &mut ClientSession,
    gesture: u32,
    phase: TouchPhase,
    x: f32,
    y: f32,
    channel: u8,
    velocity: u8,
) -> Response {
    let touch = LatestTouch {
        owner: gesture,
        x,
        y,
        channel,
        velocity,
    }
    .clamp();
    match phase {
        TouchPhase::Move => {
            control.latest.publish(touch);
            Response::Ok
        }
        TouchPhase::Down => {
            session.track_gesture(gesture);
            control.latest.publish(touch);
            let (qx, qy) = pack_xy(touch.x, touch.y);
            if send_or_drop(
                control,
                cache,
                Command::TouchDown {
                    owner: gesture,
                    x: qx,
                    y: qy,
                    channel: touch.channel,
                    velocity: touch.velocity,
                },
            ) {
                Response::Ok
            } else {
                Response::Error {
                    message: "command ring full".to_string(),
                }
            }
        }
        TouchPhase::Up | TouchPhase::Cancel => {
            control.latest.clear(gesture);
            session.untrack_gesture(gesture);
            let command = if phase == TouchPhase::Cancel {
                Command::TouchCancel { owner: gesture }
            } else {
                Command::TouchUp { owner: gesture }
            };
            if send_or_drop(control, cache, command) {
                Response::Ok
            } else {
                Response::Error {
                    message: "command ring full".to_string(),
                }
            }
        }
    }
}

fn handle_repeat(
    control: &mut ControlSide,
    cache: &mut StatusCache,
    session: &mut ClientSession,
    gesture: u32,
    phase: RepeatPhase,
    note: u8,
    channel: u8,
    velocity: u8,
    division: jambox_core::RepeatDivision,
) -> Response {
    match phase {
        RepeatPhase::Down => {
            session.track_repeat(gesture);
            if send_or_drop(
                control,
                cache,
                Command::StartRepeat {
                    owner: gesture,
                    channel,
                    note,
                    velocity,
                    division,
                },
            ) {
                Response::Ok
            } else {
                Response::Error {
                    message: "command ring full".to_string(),
                }
            }
        }
        RepeatPhase::Up | RepeatPhase::Cancel => {
            session.untrack_repeat(gesture);
            if send_or_drop(control, cache, Command::StopRepeat { owner: gesture }) {
                Response::Ok
            } else {
                Response::Error {
                    message: "command ring full".to_string(),
                }
            }
        }
    }
}

/// Accept connections until `running` clears.
///
/// Each client gets its own thread so the kiosk holding a long-lived command
/// connection cannot starve a `status` poke from the CLI.
pub fn serve(
    endpoint: Endpoint,
    control: ControlSide,
    hub: Arc<ClientHub>,
    map: Arc<MidiMap>,
    midi_in: Arc<Mutex<MidiInSide>>,
    health: Arc<AudioHealth>,
    running: Arc<AtomicBool>,
) {
    let shared: Shared = Arc::new(Mutex::new((control, StatusCache::default())));
    info!(endpoint = %endpoint.describe(), "control: listening");

    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => {
            use std::os::unix::net::UnixListener;
            let _ = std::fs::remove_file(&path);
            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(err) => {
                    warn!(%err, path = %path.display(), "control: bind failed");
                    return;
                }
            };
            for stream in listener.incoming() {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                match stream {
                    Ok(stream) => {
                        spawn_client(stream, &shared, &hub, &map, &midi_in, &health, &running)
                    }
                    Err(err) => warn!(%err, "control: accept failed"),
                }
            }
            let _ = std::fs::remove_file(&path);
        }
        Endpoint::Tcp(addr) => {
            use std::net::TcpListener;
            let listener = match TcpListener::bind(&addr) {
                Ok(l) => l,
                Err(err) => {
                    warn!(%err, %addr, "control: bind failed");
                    return;
                }
            };
            for stream in listener.incoming() {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                match stream {
                    Ok(stream) => {
                        spawn_client(stream, &shared, &hub, &map, &midi_in, &health, &running)
                    }
                    Err(err) => warn!(%err, "control: accept failed"),
                }
            }
        }
    }
}

/// Trait alias for the two stream kinds we accept.
trait ClientStream: std::io::Read + Write + Send + 'static {
    fn duplicate(&self) -> std::io::Result<Box<dyn ClientStream>>;
}

#[cfg(unix)]
impl ClientStream for std::os::unix::net::UnixStream {
    fn duplicate(&self) -> std::io::Result<Box<dyn ClientStream>> {
        Ok(Box::new(self.try_clone()?))
    }
}

impl ClientStream for std::net::TcpStream {
    fn duplicate(&self) -> std::io::Result<Box<dyn ClientStream>> {
        Ok(Box::new(self.try_clone()?))
    }
}

impl ClientStream for Box<dyn ClientStream> {
    fn duplicate(&self) -> std::io::Result<Box<dyn ClientStream>> {
        (**self).duplicate()
    }
}

fn spawn_client<S: ClientStream>(
    stream: S,
    shared: &Shared,
    hub: &Arc<ClientHub>,
    map: &Arc<MidiMap>,
    midi_in: &Arc<Mutex<MidiInSide>>,
    health: &Arc<AudioHealth>,
    running: &Arc<AtomicBool>,
) {
    let reader = match stream.duplicate() {
        Ok(s) => s,
        Err(err) => {
            warn!(%err, "control: clone failed");
            return;
        }
    };
    let mut writer = stream;
    let (tx, rx) = hub.subscribe();
    let shared = Arc::clone(shared);
    let running = Arc::clone(running);
    let hub = Arc::clone(hub);
    let map = Arc::clone(map);
    let midi_in = Arc::clone(midi_in);
    let health = Arc::clone(health);
    std::thread::spawn(move || {
        serve_client(
            BufReader::new(reader),
            tx,
            &shared,
            &running,
            &hub,
            &map,
            &midi_in,
            &health,
        );
    });
    std::thread::spawn(move || {
        for line in rx {
            if writeln!(writer, "{line}").is_err() || writer.flush().is_err() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus;
    use jambox_core::Command;

    fn call(
        line: &str,
        control: &mut ControlSide,
        cache: &mut StatusCache,
        midi_in: &Mutex<MidiInSide>,
        session: &mut ClientSession,
    ) -> Response {
        handle_line(
            line,
            control,
            cache,
            &ClientHub::default(),
            &MidiMap::default(),
            midi_in,
            &AudioHealth::new(),
            session,
        )
    }

    #[test]
    fn a_note_request_reaches_the_audio_ring() {
        let (mut control, midi_in, _midi_out, mut audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        let mut cache = StatusCache::default();
        let mut session = ClientSession::new();
        let response = call(
            r#"{"cmd":"note_on","channel":0,"note":60,"velocity":100}"#,
            &mut control,
            &mut cache,
            &midi_in,
            &mut session,
        );
        assert!(matches!(response, Response::Ok));
        assert_eq!(
            audio.midi_commands.pop().unwrap(),
            Command::NoteOn {
                channel: 0,
                note: 60,
                velocity: 100
            }
        );
    }

    #[test]
    fn malformed_json_is_answered_not_fatal() {
        let (mut control, midi_in, _midi_out, _audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        let mut cache = StatusCache::default();
        let mut session = ClientSession::new();
        let response = call("{not json", &mut control, &mut cache, &midi_in, &mut session);
        assert!(matches!(response, Response::Error { .. }));
    }

    #[test]
    fn status_reports_what_audio_published() {
        let (mut control, midi_in, _midi_out, mut audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        audio
            .status
            .push(crate::bus::StatusPacket {
                engine: jambox_core::EngineStatus {
                    position: 4242,
                    bpm: 128.0,
                    active_voices: 3,
                    ..Default::default()
                },
                xruns: 2,
                callback_frames: 512,
                ..Default::default()
            })
            .unwrap();
        let mut cache = StatusCache::default();
        let mut session = ClientSession::new();
        match call(
            r#"{"cmd":"status"}"#,
            &mut control,
            &mut cache,
            &midi_in,
            &mut session,
        ) {
            Response::Status(reply) => {
                assert_eq!(reply.position, 4242);
                assert_eq!(reply.active_voices, 3);
                assert_eq!(reply.xruns, 2);
                assert_eq!(reply.callback_frames, 512);
                assert_eq!(reply.command_drops, 0);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn clip_load_is_queued_as_a_pointer_swap() {
        let (mut control, midi_in, _midi_out, mut audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        let mut cache = StatusCache::default();
        let mut session = ClientSession::new();
        let response = call(
            r#"{"cmd":"clip_load","slot":1,"length_ticks":960,
                "events":[{"tick":0,"on":true,"channel":0,"note":60,"velocity":90}]}"#,
            &mut control,
            &mut cache,
            &midi_in,
            &mut session,
        );
        assert!(matches!(response, Response::Ok));
        let update = audio.clips.pop().unwrap();
        assert_eq!(update.slot, 1);
        assert!(update.clip.is_some());
    }

    #[test]
    fn a_full_ring_is_reported_to_the_ui() {
        let (mut control, midi_in, _midi_out, _audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        let mut cache = StatusCache::default();
        let mut session = ClientSession::new();
        let line = r#"{"cmd":"panic"}"#;
        let mut saw_error = false;
        for _ in 0..2048 {
            if let Response::Error { .. } =
                call(line, &mut control, &mut cache, &midi_in, &mut session)
            {
                saw_error = true;
                break;
            }
        }
        assert!(saw_error, "backpressure must surface, not block");
        assert!(cache.dropped_commands > 0);
    }

    #[test]
    fn touch_moves_do_not_enter_the_command_ring() {
        let (mut control, midi_in, _midi_out, mut audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        let mut cache = StatusCache::default();
        let mut session = ClientSession::new();
        call(
            r#"{"cmd":"touch","gesture":9,"phase":"down","x":0.1,"y":0.8}"#,
            &mut control,
            &mut cache,
            &midi_in,
            &mut session,
        );
        for i in 0..100 {
            let x = i as f32 / 99.0;
            call(
                &format!(r#"{{"cmd":"touch","gesture":9,"phase":"move","x":{x},"y":0.8}}"#),
                &mut control,
                &mut cache,
                &midi_in,
                &mut session,
            );
        }
        call(
            r#"{"cmd":"touch","gesture":9,"phase":"up","x":0.9,"y":0.8}"#,
            &mut control,
            &mut cache,
            &midi_in,
            &mut session,
        );
        assert!(matches!(
            audio.control_commands.pop().unwrap(),
            Command::TouchDown { owner: 9, .. }
        ));
        assert_eq!(
            audio.control_commands.pop().unwrap(),
            Command::TouchUp { owner: 9 }
        );
        assert!(audio.control_commands.pop().is_err());
        let mut latest = [LatestTouch {
            owner: 0,
            x: 0.0,
            y: 0.0,
            channel: 0,
            velocity: 0,
        }; 8];
        assert_eq!(control.latest.snapshot(&mut latest), 0);
    }

    #[test]
    fn disconnect_cancels_owned_gestures_only() {
        let (mut control, midi_in, _midi_out, mut audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        let mut cache = StatusCache::default();
        let mut session = ClientSession::new();
        call(
            r#"{"cmd":"hello","protocol":1,"client":"slice","realtime_owner":true}"#,
            &mut control,
            &mut cache,
            &midi_in,
            &mut session,
        );
        call(
            r#"{"cmd":"touch","gesture":4,"phase":"down","x":0.2,"y":0.5}"#,
            &mut control,
            &mut cache,
            &midi_in,
            &mut session,
        );
        call(
            r#"{"cmd":"repeat","gesture":41,"phase":"down","note":36}"#,
            &mut control,
            &mut cache,
            &midi_in,
            &mut session,
        );
        let _ = audio.control_commands.pop();
        let _ = audio.control_commands.pop();
        emergency_release(&mut control, &mut cache, &mut session);
        assert_eq!(cache.emergency_releases, 1);
        let mut saw_cancel = false;
        let mut saw_stop = false;
        while let Ok(cmd) = audio.control_commands.pop() {
            match cmd {
                Command::TouchCancel { owner: 4 } => saw_cancel = true,
                Command::StopRepeat { owner: 41 } => saw_stop = true,
                _ => {}
            }
        }
        assert!(saw_cancel && saw_stop);
        assert!(session.gestures.is_empty());
    }

    #[test]
    fn audio_reopen_sets_health_flag() {
        let (mut control, midi_in, _midi_out, _audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        let mut cache = StatusCache::default();
        let mut session = ClientSession::new();
        let health = AudioHealth::new();
        let response = handle_line(
            r#"{"cmd":"audio_reopen"}"#,
            &mut control,
            &mut cache,
            &ClientHub::default(),
            &MidiMap::default(),
            &midi_in,
            &health,
            &mut session,
        );
        assert!(matches!(response, Response::Ok));
        assert!(health.take_reopen());
    }
}
