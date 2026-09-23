//! Raspberry Pi under-voltage / throttle flags (`vcgencmd get_throttled`).
//!
//! Official firmware draws a lightning bolt; the native kiosk never did.
//! Bits match the mailbox: 0 under-voltage now, 2 throttled now, 16/18 sticky.

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::path::Path;

pub const UV_NOW: u32 = 1 << 0;
pub const FREQ_CAP_NOW: u32 = 1 << 1;
pub const THROTTLED_NOW: u32 = 1 << 2;
pub const SOFT_TEMP_NOW: u32 = 1 << 3;
pub const UV_OCCURRED: u32 = 1 << 16;
pub const FREQ_CAP_OCCURRED: u32 = 1 << 17;
pub const THROTTLED_OCCURRED: u32 = 1 << 18;

#[cfg(target_os = "linux")]
const SYSFS_CANDIDATES: &[&str] = &[
    "/sys/devices/platform/soc/soc:firmware/get_throttled",
    "/sys/devices/platform/firmware:raspberrypi-firmware/get_throttled",
    "/sys/class/hwmon/hwmon0/device/get_throttled",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThrottleState {
    pub flags: u32,
}

impl ThrottleState {
    pub fn from_flags(flags: u32) -> Self {
        Self { flags }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        let t = t
            .strip_prefix("throttled=")
            .or_else(|| t.strip_prefix("throttled ="))
            .unwrap_or(t)
            .trim();
        if t.is_empty() {
            return None;
        }
        if let Some(hex) = t
            .strip_prefix("0x")
            .or_else(|| t.strip_prefix("0X"))
        {
            return u32::from_str_radix(hex.trim(), 16)
                .ok()
                .map(Self::from_flags);
        }
        t.parse::<u32>().ok().map(Self::from_flags)
    }

    pub fn undervolt_now(self) -> bool {
        self.flags & UV_NOW != 0
    }

    pub fn throttled_now(self) -> bool {
        self.flags & THROTTLED_NOW != 0
    }

    pub fn undervolt_occurred(self) -> bool {
        self.flags & (UV_NOW | UV_OCCURRED) != 0
    }

    pub fn power_fault_now(self) -> bool {
        self.undervolt_now() || self.throttled_now()
    }

    pub fn power_fault(self) -> bool {
        self.power_fault_now() || self.undervolt_occurred() || (self.flags & THROTTLED_OCCURRED != 0)
    }

    pub fn badge_label(self) -> Option<&'static str> {
        if self.undervolt_now() {
            Some("LOW PWR")
        } else if self.throttled_now() {
            Some("THROTTLE")
        } else if self.undervolt_occurred() {
            Some("LOW PWR")
        } else if self.flags & THROTTLED_OCCURRED != 0 {
            Some("THROTTLE")
        } else {
            None
        }
    }

    pub fn log_line(self) -> String {
        if self.undervolt_now() {
            format!("LOW POWER — supply sag now (throttled=0x{:x})", self.flags)
        } else if self.throttled_now() {
            format!("THROTTLED — CPU capped now (throttled=0x{:x})", self.flags)
        } else {
            format!(
                "LOW POWER this boot (throttled=0x{:x}) — check the PSU / USB cable",
                self.flags
            )
        }
    }
}

/// Live mailbox / hwmon / `vcgencmd`. `PIDI_THROTTLED` overrides for tests.
pub fn read_throttle() -> ThrottleState {
    if let Ok(raw) = std::env::var("PIDI_THROTTLED") {
        if let Some(state) = ThrottleState::parse(&raw) {
            return state;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(state) = read_sysfs() {
            return state;
        }
        if let Some(state) = read_hwmon_alarm() {
            return state;
        }
        if let Some(state) = read_vcgencmd() {
            return state;
        }
    }
    ThrottleState::default()
}

#[cfg(target_os = "linux")]
fn read_sysfs() -> Option<ThrottleState> {
    for path in SYSFS_CANDIDATES {
        if let Ok(text) = fs::read_to_string(path) {
            if let Some(state) = ThrottleState::parse(&text) {
                return Some(state);
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn read_hwmon_alarm() -> Option<ThrottleState> {
    let root = Path::new("/sys/class/hwmon");
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let alarm = entry.path().join("in0_lcrit_alarm");
        let Ok(text) = fs::read_to_string(&alarm) else {
            continue;
        };
        let v = text.trim();
        if v == "1" || v.eq_ignore_ascii_case("true") {
            return Some(ThrottleState::from_flags(UV_NOW | UV_OCCURRED));
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn read_vcgencmd() -> Option<ThrottleState> {
    let out = std::process::Command::new("vcgencmd")
        .arg("get_throttled")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    ThrottleState::parse(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vcgencmd_hex() {
        let s = ThrottleState::parse("throttled=0x50005").unwrap();
        assert!(s.undervolt_now());
        assert!(s.throttled_now());
        assert!(s.undervolt_occurred());
        assert_eq!(s.badge_label(), Some("LOW PWR"));
    }

    #[test]
    fn parses_bare_hex() {
        let s = ThrottleState::parse("0x50000").unwrap();
        assert!(!s.undervolt_now());
        assert!(s.undervolt_occurred());
        assert!(s.power_fault());
    }

    #[test]
    fn healthy_is_quiet() {
        let s = ThrottleState::parse("0x0").unwrap();
        assert!(!s.power_fault());
        assert!(s.badge_label().is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(ThrottleState::parse("").is_none());
        assert!(ThrottleState::parse("nope").is_none());
    }
}
