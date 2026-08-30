//! Phrase pad files (`pad-01.json` … `pad-16.json`) — same on-disk shape as the old kiosk.

use std::fs;
use std::path::{Path, PathBuf};

use jambox_protocol::WireClipEvent;
use serde::{Deserialize, Serialize};

/// Ticks per quarter note — must match `jambox_core::PPQ`.
pub const PPQ: u32 = 960;
pub const PHRASE_GAIN_STEP: f32 = 0.1;

#[derive(Debug, Clone)]
pub struct PhrasePad {
    pub empty: bool,
    pub loop_mode: bool,
    pub length_ticks: u32,
    pub length_secs: f64,
    pub events: Vec<WireClipEvent>,
    pub gain: f32,
    pub voice_locked: bool,
    pub morph_a: u16,
    pub morph_b: u16,
    pub morph: f32,
    pub out_channel: i8, // -1 = as recorded
    pub local_synth: bool,
}

impl Default for PhrasePad {
    fn default() -> Self {
        Self {
            empty: true,
            loop_mode: false,
            length_ticks: 0,
            length_secs: 0.0,
            events: Vec::new(),
            gain: 1.0,
            voice_locked: false,
            morph_a: 0,
            morph_b: 1,
            morph: 0.5,
            out_channel: -1,
            local_synth: true,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FilePhrase {
    #[serde(default = "version_default")]
    version: u32,
    #[serde(default)]
    length: f64,
    #[serde(default)]
    trigger_mode: String,
    #[serde(default)]
    voice_mode: String,
    #[serde(default)]
    morph_a: String,
    #[serde(default)]
    morph_b: String,
    #[serde(default)]
    morph: f32,
    #[serde(default = "out_channel_default")]
    out_channel: i8,
    #[serde(default = "true_default")]
    local_synth: bool,
    #[serde(default = "gain_default")]
    gain: f32,
    #[serde(default)]
    events: Vec<FileEvent>,
}

fn version_default() -> u32 {
    4
}
fn out_channel_default() -> i8 {
    -1
}
fn true_default() -> bool {
    true
}
fn gain_default() -> f32 {
    1.0
}

#[derive(Debug, Serialize, Deserialize)]
struct FileEvent {
    t: f64,
    on: bool,
    channel: u8,
    note: u8,
    #[serde(default)]
    velocity: u8,
}

pub fn phrases_dir_from_env() -> PathBuf {
    crate::paths::phrases_dir()
}

pub fn pad_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("pad-{:02}.json", index + 1))
}

pub fn load_bank(dir: &Path, bpm: f32) -> [PhrasePad; 16] {
    let mut out = std::array::from_fn(|_| PhrasePad::default());
    for i in 0..16 {
        out[i] = load_pad(&pad_path(dir, i), bpm).unwrap_or_default();
    }
    out
}

pub fn load_pad(path: &Path, bpm: f32) -> Option<PhrasePad> {
    let raw = fs::read_to_string(path).ok()?;
    let file: FilePhrase = serde_json::from_str(&raw).ok()?;
    let bpm = bpm.max(1.0);
    let gain = file.gain.clamp(0.1, 2.0);
    let mut events: Vec<WireClipEvent> = file
        .events
        .iter()
        .map(|e| WireClipEvent {
            tick: seconds_to_ticks(e.t, bpm),
            on: e.on,
            channel: e.channel & 0x0f,
            note: e.note & 0x7f,
            velocity: scale_velocity(e.velocity, gain),
        })
        .collect();
    events.sort_by_key(|e| e.tick);
    let length_secs = if file.length > 0.0 {
        file.length
    } else {
        file.events
            .iter()
            .map(|e| e.t)
            .fold(0.0_f64, f64::max)
            + 0.05
    };
    let length_ticks = seconds_to_ticks(length_secs, bpm);
    let empty = events.is_empty() || length_ticks == 0;
    Some(PhrasePad {
        empty,
        loop_mode: file.trigger_mode.eq_ignore_ascii_case("loop"),
        length_ticks,
        length_secs,
        events,
        gain,
        voice_locked: file.voice_mode.eq_ignore_ascii_case("locked"),
        morph_a: file.morph_a.parse().unwrap_or(0),
        morph_b: file.morph_b.parse().unwrap_or(1),
        morph: file.morph.clamp(0.0, 1.0),
        out_channel: file.out_channel.clamp(-1, 15),
        local_synth: file.local_synth,
    })
}

pub fn save_pad(dir: &Path, index: usize, pad: &PhrasePad, bpm: f32) -> bool {
    if let Err(err) = fs::create_dir_all(dir) {
        tracing::warn!(%err, "phrases: mkdir failed");
        return false;
    }
    let bpm = bpm.max(1.0);
    // Store unscaled velocities (divide gain back out) so gain remains editable.
    let events: Vec<FileEvent> = pad
        .events
        .iter()
        .map(|e| FileEvent {
            t: ticks_to_seconds(e.tick, bpm),
            on: e.on,
            channel: e.channel,
            note: e.note,
            velocity: unscale_velocity(e.velocity, pad.gain),
        })
        .collect();
    let file = FilePhrase {
        version: 4,
        length: if pad.length_secs > 0.0 {
            pad.length_secs
        } else {
            ticks_to_seconds(pad.length_ticks, bpm)
        },
        trigger_mode: if pad.loop_mode {
            "loop".into()
        } else {
            "oneshot".into()
        },
        voice_mode: if pad.voice_locked {
            "locked".into()
        } else {
            "follow".into()
        },
        morph_a: pad.morph_a.to_string(),
        morph_b: pad.morph_b.to_string(),
        morph: pad.morph,
        out_channel: pad.out_channel,
        local_synth: pad.local_synth,
        gain: pad.gain.clamp(0.1, 2.0),
        events,
    };
    let path = pad_path(dir, index);
    match serde_json::to_string_pretty(&file) {
        Ok(body) => fs::write(path, body + "\n").is_ok(),
        Err(_) => false,
    }
}

pub fn delete_pad(dir: &Path, index: usize) -> bool {
    let path = pad_path(dir, index);
    if path.is_file() {
        fs::remove_file(path).is_ok()
    } else {
        true
    }
}

/// Build a pad from wire clip events (SEQ → PAD, live record).
pub fn from_wire(
    events: Vec<WireClipEvent>,
    length_ticks: u32,
    bpm: f32,
    loop_mode: bool,
) -> PhrasePad {
    let bpm = bpm.max(1.0);
    let length_ticks = length_ticks.max(1);
    let mut events = events;
    events.sort_by_key(|e| e.tick);
    let empty = events.is_empty();
    PhrasePad {
        empty,
        loop_mode,
        length_ticks,
        length_secs: ticks_to_seconds(length_ticks, bpm),
        events,
        ..PhrasePad::default()
    }
}

pub fn seconds_to_ticks(seconds: f64, bpm: f32) -> u32 {
    let beats = seconds.max(0.0) * (f64::from(bpm) / 60.0);
    (beats * f64::from(PPQ)).round().clamp(0.0, u32::MAX as f64) as u32
}

pub fn ticks_to_seconds(ticks: u32, bpm: f32) -> f64 {
    let beats = f64::from(ticks) / f64::from(PPQ);
    beats * (60.0 / f64::from(bpm.max(1.0)))
}

pub fn scale_velocity(vel: u8, gain: f32) -> u8 {
    if vel == 0 {
        return 0;
    }
    ((f32::from(vel) * gain.clamp(0.1, 2.0)).round() as u32)
        .clamp(1, 127) as u8
}

fn unscale_velocity(vel: u8, gain: f32) -> u8 {
    if vel == 0 {
        return 0;
    }
    let g = gain.clamp(0.1, 2.0);
    ((f32::from(vel) / g).round() as u32).clamp(1, 127) as u8
}

pub fn pad_label(cell: usize) -> String {
    let c = cell.min(15);
    let bank = if c < 8 { 'A' } else { 'B' };
    format!("{}{}", bank, (c % 8) + 1)
}

/// Factory MPK pad notes start at 36.
pub const PHRASE_PAD_BASE: u8 = 36;

/// Screen 4×4 order (top→bottom): A1–A4, A5–A8, B1–B4, B5–B8.
pub const PHRASE_GRID_CELLS: [usize; 16] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

fn mpk_row_swap(within_bank: usize) -> usize {
    (within_bank + 4) & 7
}

/// Phrase cell 0..15 → factory MPK note (row-swapped to match the screen).
pub fn mpk_note_for_phrase_cell(cell: usize) -> u8 {
    let c = cell & 0x0F;
    let bank = c & !7;
    PHRASE_PAD_BASE + bank as u8 + mpk_row_swap(c & 7) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn seconds_to_ticks_at_120_bpm() {
        assert_eq!(seconds_to_ticks(0.5, 120.0), 960);
    }

    #[test]
    fn mpk_note_row_swap_matches_tk() {
        assert_eq!(mpk_note_for_phrase_cell(0), 40);
        assert_eq!(mpk_note_for_phrase_cell(4), 36);
        assert_eq!(pad_label(0), "A1");
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
        assert_eq!(pad.length_ticks, 1920);
        assert!(save_pad(&dir, 0, &pad, 120.0));
        let round = load_pad(&path, 120.0).unwrap();
        assert_eq!(round.events.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_wire_round_trips_length() {
        let pad = from_wire(
            vec![WireClipEvent {
                tick: 0,
                on: true,
                channel: 9,
                note: 36,
                velocity: 100,
            }],
            1920,
            120.0,
            true,
        );
        assert!((pad.length_secs - 1.0).abs() < 0.001);
        assert!(pad.loop_mode);
    }
}
