//! Cheap local probe log for audio-choke diagnosis.
//!
//! One line every couple of seconds while SET→PROBE is on. No network, no
//! extra threads, no allocations on the audio path.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths;

/// Seconds between file samples while probe is enabled.
pub const INTERVAL_SEC: f32 = 2.0;

/// Rotate (truncate) when the log grows past this size.
const MAX_BYTES: u64 = 256 * 1024;

pub fn log_path() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_PROBE_LOG") {
        let p = p.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    paths::data_root().join("probe.log")
}

/// First field of `/proc/loadavg` when present (Pi). Host tests get `None`.
pub fn load_avg() -> Option<f32> {
    let raw = fs::read_to_string("/proc/loadavg").ok()?;
    raw.split_whitespace().next()?.parse().ok()
}

pub fn format_line(
    connected: bool,
    reconnects: u64,
    callback_frames: u32,
    callback_micros: u32,
    callback_peak_micros: u32,
    xruns: u64,
    command_drops: u64,
    emergency_releases: u64,
    active_voices: u16,
    active_drums: u16,
    playing_clips: u16,
    peak: f32,
    throttle: &str,
) -> String {
    let load = load_avg()
        .map(|v| format!("{v:.2}"))
        .unwrap_or_else(|| "-".into());
    format!(
        "{} load={load} link={} recon={reconnects} cb={callback_frames}/{callback_micros}us peakus={callback_peak_micros} xrun={xruns} drop={command_drops} rel={emergency_releases} vo={active_voices} drm={active_drums} clip={playing_clips} pk={peak:.3} {throttle}",
        unix_stamp(),
        if connected { "up" } else { "down" },
    )
}

/// Append one line. Rotates by truncating if the file is already large.
pub fn append(line: &str) -> bool {
    let path = log_path();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    let too_big = fs::metadata(&path)
        .map(|m| m.len() >= MAX_BYTES)
        .unwrap_or(false);
    let mut opts = OpenOptions::new();
    opts.create(true).write(true);
    if too_big {
        opts.truncate(true);
    } else {
        opts.append(true);
    }
    let Ok(mut file) = opts.open(&path) else {
        return false;
    };
    if too_big {
        let _ = writeln!(file, "{} rotated (was >{MAX_BYTES} bytes)", unix_stamp());
    }
    writeln!(file, "{line}").is_ok()
}

fn unix_stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_line_includes_xrun_and_reconnect() {
        let line = format_line(
            true, 2, 512, 180, 210, 3, 1, 0, 4, 1, 2, 0.25, "uv=0",
        );
        assert!(line.contains("link=up"));
        assert!(line.contains("recon=2"));
        assert!(line.contains("xrun=3"));
        assert!(line.contains("drop=1"));
        assert!(line.contains("cb=512/180us"));
        assert!(line.contains("uv=0"));
    }

    #[test]
    fn append_writes_and_survives_missing_parent() {
        let _g = crate::probe::ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "pidi-probe-{}-{}",
            std::process::id(),
            unix_stamp()
        ));
        let path = dir.join("nested").join("probe.log");
        std::env::set_var("PIDI_PROBE_LOG", &path);
        assert!(append("hello probe"));
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("hello probe"));
        let _ = fs::remove_dir_all(&dir);
        std::env::remove_var("PIDI_PROBE_LOG");
    }
}
