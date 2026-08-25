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

pub fn delete_song(path: &Path) -> bool {
    if path.is_file() {
        fs::remove_file(path).is_ok()
    } else {
        true
    }
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

fn write_vlq(mut value: u32, out: &mut Vec<u8>) {
    let mut stack = [0u8; 5];
    let mut n = 0;
    stack[n] = (value & 0x7f) as u8;
    n += 1;
    value >>= 7;
    while value > 0 {
        stack[n] = (value & 0x7f) as u8 | 0x80;
        n += 1;
        value >>= 7;
    }
    for i in (0..n).rev() {
        out.push(stack[i]);
    }
}

/// Write a Type-0 SMF from clip events (ticks at `PPQ`).
pub fn write_smf_type0(
    path: &Path,
    events: &[WireClipEvent],
    length_ticks: u32,
    bpm: f32,
) -> bool {
    let bpm = bpm.clamp(20.0, 400.0);
    let us_per_beat = (60_000_000.0 / bpm).round() as u32;
    let mut track = Vec::new();
    // set_tempo
    track.push(0x00);
    track.extend_from_slice(&[0xff, 0x51, 0x03]);
    track.push(((us_per_beat >> 16) & 0xff) as u8);
    track.push(((us_per_beat >> 8) & 0xff) as u8);
    track.push((us_per_beat & 0xff) as u8);

    let mut ordered: Vec<&WireClipEvent> = events.iter().collect();
    ordered.sort_by(|a, b| {
        a.tick
            .cmp(&b.tick)
            .then_with(|| (!a.on).cmp(&(!b.on)) /* offs before ons */)
    });

    let mut last_tick = 0u32;
    for ev in ordered {
        let delta = ev.tick.saturating_sub(last_tick);
        last_tick = ev.tick;
        write_vlq(delta, &mut track);
        let status = if ev.on {
            0x90 | (ev.channel & 0x0f)
        } else {
            0x80 | (ev.channel & 0x0f)
        };
        track.push(status);
        track.push(ev.note & 0x7f);
        track.push(if ev.on {
            ev.velocity.max(1).min(127)
        } else {
            0
        });
    }
    let pad = length_ticks.saturating_sub(last_tick);
    write_vlq(pad, &mut track);
    track.extend_from_slice(&[0xff, 0x2f, 0x00]);

    let mut data = Vec::new();
    data.extend_from_slice(b"MThd");
    data.extend_from_slice(&6u32.to_be_bytes());
    data.extend_from_slice(&0u16.to_be_bytes()); // type 0
    data.extend_from_slice(&1u16.to_be_bytes()); // one track
    data.extend_from_slice(&(PPQ as u16).to_be_bytes());
    data.extend_from_slice(b"MTrk");
    data.extend_from_slice(&(track.len() as u32).to_be_bytes());
    data.extend_from_slice(&track);

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("mid.tmp");
    if fs::write(&tmp, &data).is_err() {
        return false;
    }
    let _ = fs::remove_file(path);
    if fs::rename(&tmp, path).is_ok() {
        return true;
    }
    // Fallback if rename across volumes fails.
    let ok = fs::write(path, &data).is_ok();
    let _ = fs::remove_file(&tmp);
    ok
}

/// Next `seq-YYYYMMDD-HHMMSS.mid` (or `seq-export-N.mid` fallback).
pub fn next_seq_export_path(dir: &Path) -> PathBuf {
    let _ = fs::create_dir_all(dir);
    let stamp = chrono_like_stamp();
    if !stamp.is_empty() {
        let candidate = dir.join(format!("seq-{stamp}.mid"));
        if !candidate.exists() {
            return candidate;
        }
    }
    for n in 1..10_000 {
        let candidate = dir.join(format!("seq-export-{n}.mid"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join("seq-export.mid")
}

fn chrono_like_stamp() -> String {
    // Local wall clock without pulling chrono crate: use system time via formatting
    // that works on Windows/Linux for export filenames.
    use std::time::{SystemTime, UNIX_EPOCH};
    let Ok(dur) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    let secs = dur.as_secs() as i64;
    // Approximate UTC → local is fine for a unique filename; use a fixed offset-free
    // civil date via a compact algorithm (Howard Hinnant).
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400) as u32;
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

/// Days since Unix epoch → (year, month, day) UTC.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
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

    #[test]
    fn round_trips_type0_write() {
        let dir = std::env::temp_dir().join(format!(
            "pidi-smf-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("out.mid");
        let events = vec![
            WireClipEvent {
                tick: 0,
                on: true,
                channel: 0,
                note: 60,
                velocity: 100,
            },
            WireClipEvent {
                tick: 480,
                on: false,
                channel: 0,
                note: 60,
                velocity: 0,
            },
        ];
        assert!(write_smf_type0(&path, &events, 960, 120.0));
        let (loaded, len, bpm) = load_smf_as_clip(&path).expect("reload");
        assert_eq!(loaded.len(), 2);
        assert!(loaded[0].on);
        assert_eq!(loaded[0].note, 60);
        assert!(len >= 480);
        assert!((bpm - 120.0).abs() < 0.5);
        let _ = fs::remove_dir_all(&dir);
    }
}
