//! TFT idle blanking and burn-in protection (parity with `apps/pidi/pidi/screensaver.py`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const DEFAULT_TIMEOUT_SEC: f32 = 180.0;
pub const TIMEOUT_PRESETS: [f32; 4] = [60.0, 180.0, 600.0, 0.0];
pub const PIXEL_SHIFT_AMPLITUDE: i32 = 2;
pub const PIXEL_SHIFT_DWELL_SEC: f32 = 40.0;

pub fn timeout_from_env() -> f32 {
    std::env::var("MIDI_TONE_SCREENSAVER_SEC")
        .ok()
        .and_then(|raw| raw.trim().parse::<f32>().ok())
        .map(|v| v.max(0.0))
        .unwrap_or(DEFAULT_TIMEOUT_SEC)
}

pub fn next_timeout_preset(current: f32) -> f32 {
    for (i, value) in TIMEOUT_PRESETS.iter().enumerate() {
        if (*value - current).abs() < 0.5 {
            return TIMEOUT_PRESETS[(i + 1) % TIMEOUT_PRESETS.len()];
        }
    }
    TIMEOUT_PRESETS[0]
}

pub fn timeout_label(seconds: f32) -> &'static str {
    if seconds <= 0.0 {
        return "BLANK OFF";
    }
    let minutes = seconds / 60.0;
    if (minutes - minutes.round()).abs() < 0.05 && minutes.round() >= 1.0 {
        return match minutes.round() as i32 {
            1 => "BLANK 1 MIN",
            3 => "BLANK 3 MIN",
            10 => "BLANK 10 MIN",
            n => {
                let _ = n;
                "BLANK MIN"
            }
        };
    }
    match seconds.round() as i32 {
        60 => "BLANK 1 MIN",
        180 => "BLANK 3 MIN",
        600 => "BLANK 10 MIN",
        n if n > 0 => "BLANK SEC",
        _ => "BLANK OFF",
    }
}

pub fn timeout_label_dynamic(seconds: f32) -> String {
    if seconds <= 0.0 {
        return "BLANK OFF".into();
    }
    let minutes = seconds / 60.0;
    if (minutes - minutes.round()).abs() < 0.05 && minutes.round() >= 1.0 {
        return format!("BLANK {} MIN", minutes.round() as i32);
    }
    format!("BLANK {}s", seconds.round() as i32)
}

pub fn pixel_shift_xy(elapsed: f32) -> (i32, i32) {
    let amp = PIXEL_SHIFT_AMPLITUDE.max(1);
    let dwell = PIXEL_SHIFT_DWELL_SEC.max(0.1);
    let idx = (elapsed.max(0.0) / dwell) as usize % 8;
    let pattern = [
        (0, 0),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
    ];
    let (dx, dy) = pattern[idx];
    (dx * amp, dy * amp)
}

pub struct IdleWatch {
    pub timeout_sec: f32,
    last_activity: Instant,
    pub active: bool,
}

impl IdleWatch {
    pub fn new(timeout_sec: f32) -> Self {
        Self {
            timeout_sec: timeout_sec.max(0.0),
            last_activity: Instant::now(),
            active: false,
        }
    }

    pub fn poke(&mut self) -> bool {
        self.last_activity = Instant::now();
        if self.active {
            self.active = false;
            return true;
        }
        false
    }

    pub fn due(&self) -> bool {
        if self.active || self.timeout_sec <= 0.0 {
            return false;
        }
        self.last_activity.elapsed() >= Duration::from_secs_f32(self.timeout_sec)
    }

    pub fn activate(&mut self) -> bool {
        if self.active {
            return false;
        }
        self.active = true;
        true
    }
}

pub struct PanelBacklight {
    root: PathBuf,
    brightness: Option<PathBuf>,
    saved_brightness: Option<u32>,
}

impl Default for PanelBacklight {
    fn default() -> Self {
        Self::new()
    }
}

impl PanelBacklight {
    pub fn new() -> Self {
        Self {
            root: PathBuf::from("/sys/class/backlight"),
            brightness: None,
            saved_brightness: None,
        }
    }

    fn find_brightness(&mut self) -> Option<&Path> {
        if self
            .brightness
            .as_ref()
            .is_some_and(|p| p.is_file())
        {
            return self.brightness.as_deref();
        }
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return None;
        };
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("brightness"))
            .filter(|p| p.is_file())
            .collect();
        candidates.sort();
        self.brightness = candidates.into_iter().next();
        self.brightness.as_deref()
    }

    pub fn dim(&mut self) -> bool {
        let Some(path) = self.find_brightness().map(Path::to_path_buf) else {
            return false;
        };
        let current = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        if self.saved_brightness.is_none() {
            self.saved_brightness = Some(if current > 0 {
                current
            } else {
                self.max_brightness(&path)
            });
        }
        std::fs::write(&path, "0\n").is_ok()
    }

    pub fn restore(&mut self) -> bool {
        let Some(path) = self.find_brightness().map(Path::to_path_buf) else {
            return false;
        };
        let power = path.parent().map(|p| p.join("bl_power"));
        if let Some(power) = power.as_ref().filter(|p| p.is_file()) {
            let _ = std::fs::write(power, "0\n");
        }
        let value = self
            .saved_brightness
            .filter(|v| *v > 0)
            .unwrap_or_else(|| self.max_brightness(&path));
        let ok = std::fs::write(&path, format!("{value}\n")).is_ok();
        self.saved_brightness = None;
        ok
    }

    fn max_brightness(&self, path: &Path) -> u32 {
        path.parent()
            .and_then(|p| {
                std::fs::read_to_string(p.join("max_brightness"))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
            })
            .filter(|v| *v > 0)
            .unwrap_or(255)
    }

    pub fn ensure_lit(&mut self) -> bool {
        let Some(path) = self.find_brightness().map(Path::to_path_buf) else {
            return false;
        };
        let current = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        if current > 0 {
            return true;
        }
        self.restore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_presets_cycle() {
        assert_eq!(next_timeout_preset(180.0), 600.0);
        assert_eq!(next_timeout_preset(600.0), 0.0);
        assert_eq!(next_timeout_preset(0.0), 60.0);
    }

    #[test]
    fn pixel_shift_steps() {
        assert_eq!(pixel_shift_xy(0.0), (0, 0));
        assert_eq!(pixel_shift_xy(39.9), (0, 0));
        assert_eq!(pixel_shift_xy(40.0), (2, 0));
    }
}
