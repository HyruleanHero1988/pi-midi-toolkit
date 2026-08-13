//! Control server: line-delimited JSON over a Unix socket (or TCP on other hosts).
//!
//! This thread owns all parsing and allocation. It can block, log, and touch the
//! filesystem freely — none of that reaches the audio callback.

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use jambox_core::EngineStatus;
use tracing::{debug, info, warn};

use crate::bus::{ClipUpdate, ControlSide};
use crate::protocol::{decode, Decoded, Request, Response, StatusReply};

/// Control-plane state shared by client connections.
///
/// A lock here is fine and deliberate: this is the *non*-realtime side. The audio
/// thread never touches it — it only pops from the rings this feeds.
type Shared = Arc<Mutex<(ControlSide, StatusCache)>>;

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
fn serve_client<R: BufRead, W: Write>(
    reader: R,
    mut writer: W,
    shared: &Shared,
    running: &AtomicBool,
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
            handle_line(trimmed, control, cache)
        };
        if let Ok(json) = serde_json::to_string(&response) {
            if writeln!(writer, "{json}").is_err() || writer.flush().is_err() {
                break;
            }
        }
    }
}

/// Parse + dispatch one request. Kept separate so it is directly testable.
pub fn handle_line(line: &str, control: &mut ControlSide, cache: &mut StatusCache) -> Response {
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
pub fn serve(endpoint: Endpoint, control: ControlSide, running: Arc<AtomicBool>) {
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
                    Ok(stream) => spawn_client(stream, &shared, &running),
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
                    Ok(stream) => spawn_client(stream, &shared, &running),
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

fn spawn_client<S: ClientStream>(stream: S, shared: &Shared, running: &Arc<AtomicBool>) {
    let reader = match stream.duplicate() {
        Ok(s) => s,
        Err(err) => {
            warn!(%err, "control: clone failed");
            return;
        }
    };
    let shared = Arc::clone(shared);
    let running = Arc::clone(running);
    std::thread::spawn(move || {
        serve_client(BufReader::new(reader), stream, &shared, &running);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus;
    use jambox_core::Command;

    #[test]
    fn a_note_request_reaches_the_audio_ring() {
        let (mut control, _midi_in, _midi_out, mut audio) = bus::channel();
        let mut cache = StatusCache::default();
        let response = handle_line(
            r#"{"cmd":"note_on","channel":0,"note":60,"velocity":100}"#,
            &mut control,
            &mut cache,
        );
        assert!(matches!(response, Response::Ok));
        assert_eq!(
            audio.control_commands.pop().unwrap(),
            Command::NoteOn {
                channel: 0,
                note: 60,
                velocity: 100
            }
        );
    }

    #[test]
    fn malformed_json_is_answered_not_fatal() {
        let (mut control, _midi_in, _midi_out, _audio) = bus::channel();
        let mut cache = StatusCache::default();
        let response = handle_line("{not json", &mut control, &mut cache);
        assert!(matches!(response, Response::Error { .. }));
    }

    #[test]
    fn status_reports_what_audio_published() {
        let (mut control, _midi_in, _midi_out, mut audio) = bus::channel();
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
        match handle_line(r#"{"cmd":"status"}"#, &mut control, &mut cache) {
            Response::Status(reply) => {
                assert_eq!(reply.position, 4242);
                assert_eq!(reply.active_voices, 3);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn clip_load_is_queued_as_a_pointer_swap() {
        let (mut control, _midi_in, _midi_out, mut audio) = bus::channel();
        let mut cache = StatusCache::default();
        let response = handle_line(
            r#"{"cmd":"clip_load","slot":1,"length_ticks":960,
                "events":[{"tick":0,"on":true,"channel":0,"note":60,"velocity":90}]}"#,
            &mut control,
            &mut cache,
        );
        assert!(matches!(response, Response::Ok));
        let update = audio.clips.pop().unwrap();
        assert_eq!(update.slot, 1);
        assert!(update.clip.is_some());
    }

    #[test]
    fn a_full_ring_is_reported_to_the_ui() {
        let (mut control, _midi_in, _midi_out, _audio) = bus::channel();
        let mut cache = StatusCache::default();
        let line = r#"{"cmd":"panic"}"#;
        let mut saw_error = false;
        for _ in 0..2048 {
            if let Response::Error { .. } = handle_line(line, &mut control, &mut cache) {
                saw_error = true;
                break;
            }
        }
        assert!(saw_error, "backpressure must surface, not block");
        assert!(cache.dropped_commands > 0);
    }
}
