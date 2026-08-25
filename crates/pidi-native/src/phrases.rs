//! Phrase pad files (`pad-01.json` … `pad-16.json`) — same on-disk shape as the old kiosk.

use std::fs;
use std::path::{Path, PathBuf};

use jambox_protocol::WireClipEvent;
use serde::Deserialize;

/// Ticks per quarter note — must match `jambox_core::PPQ`.
pub const PPQ: u32 = 960;

#[derive(Debug, Clone, Default)]
pub struct PhrasePad {
    pub empty: bool,
    pub loop_mode: bool,
    pub length_ticks: u32,
    pub events: Vec<WireClipEvent>,
}

#[derive(Debug, Deserialize)]
struct FilePhrase {
    #[serde(default)]
    length: f64,
    #[serde(default)]
    trigger_mode: String,
    #[serde(default)]
    events: Vec<FileEvent>,
}

#[derive(Debug, Deserialize)]
struct FileEvent {
    t: f64,
    on: bool,
    channel: u8,
    note: u8,
    #[serde(default)]
    velocity: u8,
}

pub fn phrases_dir_from_env() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_PHRASES_DIR") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("MIDI_TONE_PHRASES_DIR") {
        return PathBuf::from(p);
    }
    PathBuf::from("phrases")
}

pub fn load_bank(dir: &Path, bpm: f32) -> [PhrasePad; 16] {
    let mut out = std::array::from_fn(|_| PhrasePad::default());
    for i in 0..16 {
        let path = dir.join(format!("pad-{:02}.json", i + 1));
        out[i] = load_pad(&path, bpm).unwrap_or_default();
    }
    out
}

pub fn load_pad(path: &Path, bpm: f32) -> Option<PhrasePad> {
    let raw = fs::read_to_string(path).ok()?;
    let file: FilePhrase = serde_json::from_str(&raw).ok()?;
    let bpm = bpm.max(1.0);
    let mut events: Vec<WireClipEvent> = file
        .events
        .iter()
        .map(|e| WireClipEvent {
            tick: seconds_to_ticks(e.t, bpm),
            on: e.on,
            channel: e.channel & 0x0f,
            note: e.note & 0x7f,
            velocity: e.velocity.min(127),
        })
        .collect();
    events.sort_by_key(|e| e.tick);
    let length_ticks = if file.length > 0.0 {
        seconds_to_ticks(file.length, bpm)
    } else {
        events.last().map(|e| e.tick.saturating_add(PPQ / 4)).unwrap_or(0)
    };
    let empty = events.is_empty() || length_ticks == 0;
    Some(PhrasePad {
        empty,
        loop_mode: file.trigger_mode.eq_ignore_ascii_case("loop"),
        length_ticks,
        events,
    })
}

pub fn seconds_to_ticks(seconds: f64, bpm: f32) -> u32 {
    let beats = seconds.max(0.0) * (f64::from(bpm) / 60.0);
    (beats * f64::from(PPQ)).round().clamp(0.0, u32::MAX as f64) as u32
}

pub fn pad_label(cell: usize) -> String {
    let c = cell.min(15);
    let bank = if c < 8 { 'A' } else { 'B' };
    format!("{}{}", bank, (c % 8) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn seconds_to_ticks_at_120_bpm() {
        // 1 beat at 120 BPM = 0.5s = 960 ticks
        assert_eq!(seconds_to_ticks(0.5, 120.0), 960);
    }

    #[test]
    fn loads_pad_json() {
        let dir = std::env::temp_dir().join(format!("pidi-phrase-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pad-01.json");
        let mut f = fs::File::create(&path).unwrap();
        write!(
            f,
            r#"{{"version":4,"length":1.0,"trigger_mode":"loop","events":[
              {{"t":0.0,"on":true,"channel":9,"note":36,"velocity":100}},
              {{"t":0.25,"on":false,"channel":9,"note":36,"velocity":0}}
            ]}}"#
        )
        .unwrap();
        let pad = load_pad(&path, 120.0).unwrap();
        assert!(!pad.empty);
        assert!(pad.loop_mode);
        assert_eq!(pad.events.len(), 2);
        assert_eq!(pad.events[0].note, 36);
        assert_eq!(pad.length_ticks, 1920); // 1.0s at 120 BPM = 2 beats
        let _ = fs::remove_dir_all(&dir);
    }
}
