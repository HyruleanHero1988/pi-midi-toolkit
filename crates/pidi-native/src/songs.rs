//! Song library: list `.mid` files and load Type-0/1 note tracks into a clip.

use std::fs;
use std::path::{Path, PathBuf};

use jambox_protocol::WireClipEvent;

use crate::phrases::PPQ;
use crate::seq::SEQ_CLIP_SLOT;

pub const SONG_CLIP_SLOT: u8 = SEQ_CLIP_SLOT;

pub fn songs_dir_from_env() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_SONGS_DIR") {
        return PathBuf::from(p);
    }
    PathBuf::from("songs")
}

pub fn list_songs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
                    .unwrap_or(false)
        })
        .collect();
    files.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .cmp(&b.file_name().unwrap_or_default().to_ascii_lowercase())
    });
    files
}

/// Minimal SMF reader: collects note on/off into ticks at `PPQ`.
pub fn load_smf_as_clip(path: &Path) -> Option<(Vec<WireClipEvent>, u32, f32)> {
    let data = fs::read(path).ok()?;
    parse_smf(&data)
}

fn parse_smf(data: &[u8]) -> Option<(Vec<WireClipEvent>, u32, f32)> {
    if data.len() < 14 || &data[0..4] != b"MThd" {
        return None;
    }
    let header_len = u32::from_be_bytes(data[4..8].try_into().ok()?) as usize;
    if data.len() < 8 + header_len {
        return None;
    }
    let format = u16::from_be_bytes(data[8..10].try_into().ok()?);
    let ntrks = u16::from_be_bytes(data[10..12].try_into().ok()?);
    let division = u16::from_be_bytes(data[12..14].try_into().ok()?);
    if division & 0x8000 != 0 {
        return None; // SMPTE not supported
    }
    let tpq = division.max(1) as u32;
    let mut offset = 8 + header_len;
    let mut events = Vec::new();
    let mut bpm = 120.0_f32;
    let mut max_tick = 0u32;

    for _ in 0..ntrks {
        if offset + 8 > data.len() || &data[offset..offset + 4] != b"MTrk" {
            break;
        }
        let track_len = u32::from_be_bytes(data[offset + 4..offset + 8].try_into().ok()?) as usize;
        offset += 8;
        let end = (offset + track_len).min(data.len());
        let mut tick = 0u32;
        let mut running: Option<u8> = None;
        let mut i = offset;
        while i < end {
            let start = i;
            let (delta, consumed) = read_vlq(&data[i..end])?;
            i = start + consumed;
            tick = tick.saturating_add(delta);
            if i >= end {
                break;
            }
            let mut status = data[i];
            if status < 0x80 {
                status = running?;
            } else {
                i += 1;
                running = if status < 0xf0 { Some(status) } else { None };
            }
            match status & 0xf0 {
                0x90 | 0x80 => {
                    if i + 1 >= end {
                        break;
                    }
                    let note = data[i];
                    let vel = data[i + 1];
                    i += 2;
                    let channel = status & 0x0f;
                    let on = (status & 0xf0) == 0x90 && vel > 0;
                    let our_tick = scale_tick(tick, tpq, PPQ);
                    max_tick = max_tick.max(our_tick);
                    events.push(WireClipEvent {
                        tick: our_tick,
                        on,
                        channel,
                        note: note & 0x7f,
                        velocity: if on { vel } else { 0 },
                    });
                }
                0xa0 | 0xb0 | 0xe0 => i += 2,
                0xc0 | 0xd0 => i += 1,
                0xf0 => {
                    if status == 0xff {
                        if i + 1 >= end {
                            break;
                        }
                        let meta = data[i];
                        i += 1;
                        let meta_start = i;
                        let (len, consumed) = read_vlq(&data[i..end])?;
                        i = meta_start + consumed;
                        if meta == 0x51 && len >= 3 && i + 3 <= end {
                            let us =
                                u32::from_be_bytes([0, data[i], data[i + 1], data[i + 2]]);
                            if us > 0 {
                                bpm = 60_000_000.0 / us as f32;
                            }
                        }
                        i += len as usize;
                    } else {
                        let sysex_start = i;
                        let (len, consumed) = read_vlq(&data[i..end])?;
                        i = sysex_start + consumed + len as usize;
                    }
                    running = None;
                }
                _ => break,
            }
        }
        offset = end;
        let _ = format;
    }

    events.sort_by_key(|e| e.tick);
    if events.is_empty() {
        return None;
    }
    Some((events, max_tick.max(PPQ), bpm.clamp(40.0, 240.0)))
}

fn scale_tick(tick: u32, from_tpq: u32, to_ppq: u32) -> u32 {
    ((tick as u64) * (to_ppq as u64) / (from_tpq as u64)).min(u32::MAX as u64) as u32
}

fn read_vlq(data: &[u8]) -> Option<(u32, usize)> {
    let mut value = 0u32;
    for (i, b) in data.iter().enumerate().take(4) {
        value = (value << 7) | u32::from(b & 0x7f);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_type0() {
        let mut data = Vec::new();
        data.extend_from_slice(b"MThd");
        data.extend_from_slice(&6u32.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&480u16.to_be_bytes());
        data.extend_from_slice(b"MTrk");
        let track: &[u8] = &[
            0x00, 0x90, 60, 100, // note on
            0x00, 0x80, 60, 0,   // note off
            0x00, 0xff, 0x2f, 0x00,
        ];
        data.extend_from_slice(&(track.len() as u32).to_be_bytes());
        data.extend_from_slice(track);
        let (events, len, _bpm) = parse_smf(&data).expect("parse");
        assert_eq!(events.len(), 2);
        assert!(events[0].on);
        assert_eq!(events[0].note, 60);
        assert!(len >= 1);
    }
}
