//! Control server: line-delimited JSON over a Unix socket (or TCP on other hosts).
//!
//! This thread owns all parsing and allocation. It can block, log, and touch the
//! filesystem freely — none of that reaches the audio callback.

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use jambox_core::EngineStatus;
use tracing::{debug, info, warn};

use crate::bus::{ClipUpdate, ControlSide, MidiInSide};
use crate::midi::{ingest, MidiMap};
use crate::protocol::{decode, Decoded, MidiNotice, Request, Response, StatusReply};
use midi_core::MidiEvent;
use std::sync::mpsc::{self, Sender};

type Shared = Arc<Mutex<(ControlSide, StatusCache)>>;

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
    last: Option<EngineStatus>,
    dropped_commands: u64,
}

impl StatusCache {
    fn reply(&self) -> StatusReply {
        let s = self.last.unwrap_or_default();
        StatusReply {
            position: s.position,
            bpm: s.bpm,
            active_voices: s.active_voices,
            active_drums: s.active_drums,
            playing_clips: s.playing_clips,
            peak: s.peak,
            xruns: self.dropped_commands,
        }
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
) {
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
            handle_line(trimmed, control, cache, hub, map, midi_in)
        };
        if let Ok(json) = serde_json::to_string(&response) {
            if tx.send(json).is_err() {
                break;
            }
        }
    }
}

/// Parse + dispatch one request. Kept separate so it is directly testable.
pub fn handle_line(
    line: &str,
    control: &mut ControlSide,
    cache: &mut StatusCache,
    hub: &ClientHub,
    map: &MidiMap,
    midi_in: &Mutex<MidiInSide>,
) -> Response {
    // Keep the audio thread's allocator quiet: free replaced clips here.
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
        Ok(Decoded::StatusRequest) => Response::Status(cache.reply()),
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
            if control.send(command) {
                Response::Ok
            } else {
                cache.dropped_commands += 1;
                Response::Error {
                    message: "command ring full".to_string(),
                }
            }
        }
        Ok(Decoded::ClipUpdate { slot, clip, mode }) => {
            if control.send_clip(ClipUpdate { slot, clip, mode }) {
                Response::Ok
            } else {
                cache.dropped_commands += 1;
                Response::Error {
                    message: "clip ring full".to_string(),
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
                    Ok(stream) => spawn_client(stream, &shared, &hub, &map, &midi_in, &running),
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
                    Ok(stream) => spawn_client(stream, &shared, &hub, &map, &midi_in, &running),
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
    std::thread::spawn(move || {
        serve_client(
            BufReader::new(reader),
            tx,
            &shared,
            &running,
            &hub,
            &map,
            &midi_in,
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

    fn call(line: &str, control: &mut ControlSide, cache: &mut StatusCache, midi_in: &Mutex<MidiInSide>) -> Response {
        handle_line(
            line,
            control,
            cache,
            &ClientHub::default(),
            &MidiMap::default(),
            midi_in,
        )
    }

    #[test]
    fn a_note_request_reaches_the_audio_ring() {
        let (mut control, midi_in, _midi_out, mut audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        let mut cache = StatusCache::default();
        let response = call(
            r#"{"cmd":"note_on","channel":0,"note":60,"velocity":100}"#,
            &mut control,
            &mut cache,
            &midi_in,
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
        let response = call("{not json", &mut control, &mut cache, &midi_in);
        assert!(matches!(response, Response::Error { .. }));
    }

    #[test]
    fn status_reports_what_audio_published() {
        let (mut control, midi_in, _midi_out, mut audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        audio
            .status
            .push(EngineStatus {
                position: 4242,
                bpm: 128.0,
                active_voices: 3,
                ..Default::default()
            })
            .unwrap();
        let mut cache = StatusCache::default();
        match call(r#"{"cmd":"status"}"#, &mut control, &mut cache, &midi_in) {
            Response::Status(reply) => {
                assert_eq!(reply.position, 4242);
                assert_eq!(reply.active_voices, 3);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn clip_load_is_queued_as_a_pointer_swap() {
        let (mut control, midi_in, _midi_out, mut audio) = bus::channel();
        let midi_in = Mutex::new(midi_in);
        let mut cache = StatusCache::default();
        let response = call(
            r#"{"cmd":"clip_load","slot":1,"length_ticks":960,
                "events":[{"tick":0,"on":true,"channel":0,"note":60,"velocity":90}]}"#,
            &mut control,
            &mut cache,
            &midi_in,
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
        let line = r#"{"cmd":"panic"}"#;
        let mut saw_error = false;
        for _ in 0..2048 {
            if let Response::Error { .. } = call(line, &mut control, &mut cache, &midi_in) {
                saw_error = true;
                break;
            }
        }
        assert!(saw_error, "backpressure must surface, not block");
        assert!(cache.dropped_commands > 0);
    }
}
