//! USB VBUS for the Wi-Fi dongle: on only while SET → WIFI / UPDATE is open.
//!
//! The helper [`apps/pidi/scripts/session/pi-wifi-power.sh`] cuts the LAN9514
//! port (uhubctl / sysfs). Ethernet and USB MIDI ports are never touched.

#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
#[cfg(target_os = "linux")]
use crate::host;

/// True while a panel or in-flight job still needs the radio.
pub fn usb_wanted(
    wifi_open: bool,
    wifi_kb: bool,
    update_open: bool,
    wifi_busy: bool,
    update_busy: bool,
) -> bool {
    wifi_open || wifi_kb || update_open || wifi_busy || update_busy
}

pub fn parse_sysfs_loc(loc: &str) -> Option<(String, u8)> {
    let loc = loc.trim();
    if loc.contains(':') {
        return None;
    }
    let (hub, port) = loc.rsplit_once('.')?;
    if hub.is_empty() {
        return None;
    }
    let port: u8 = port.parse().ok()?;
    Some((hub.to_string(), port))
}

#[cfg(target_os = "linux")]
fn script_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PIDI_WIFI_POWER_SCRIPT") {
        let p = PathBuf::from(p.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    let root = if let Ok(p) = std::env::var("PIDI_REPO_ROOT") {
        PathBuf::from(p)
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    };
    for cand in [
        root.join("apps/pidi/scripts/session/pi-wifi-power.sh"),
        PathBuf::from("/usr/local/sbin/pi-wifi-power.sh"),
        PathBuf::from("apps/pidi/scripts/session/pi-wifi-power.sh"),
    ] {
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Block until the dongle is powered (OTA / nmcli worker threads).
pub fn ensure(on: bool) -> (bool, String) {
    if host_dry() {
        return (true, format!("wifi-usb dry-run {}", if on { "on" } else { "off" }));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = on;
        return (true, "wifi-usb: appliance only".into());
    }
    #[cfg(target_os = "linux")]
    {
        run_script(on)
    }
}

fn host_dry() -> bool {
    #[cfg(test)]
    {
        return true;
    }
    #[cfg(not(test))]
    false
}

#[cfg(target_os = "linux")]
fn run_script(on: bool) -> (bool, String) {
    let action = if on { "on" } else { "off" };
    let Some(script) = script_path() else {
        return (
            true,
            "wifi-usb: helper missing — radio stays as NetworkManager left it".into(),
        );
    };
    let script_arg = script.to_string_lossy().to_string();
    let attempts: [&[&str]; 2] = [
        &["sudo", "-n", &script_arg, action],
        &[&script_arg, action],
    ];
    let mut last = String::new();
    for args in attempts {
        let (code, stdout, stderr) = host::run_capture(
            Command::new(args[0]).args(&args[1..]),
            if on { 20 } else { 12 },
        );
        let blob = if !stdout.is_empty() {
            stdout
        } else {
            stderr
        };
        last = blob.clone();
        if code == 0 {
            return (true, if blob.is_empty() {
                format!("wifi-usb {action}")
            } else {
                blob
            });
        }
        if code == 127 {
            continue;
        }
    }
    (false, if last.is_empty() {
        format!("wifi-usb {action} failed")
    } else {
        last
    })
}

pub fn spawn(on: bool) -> Receiver<(bool, bool, String)> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("pidi-wifi-usb".into())
        .spawn(move || {
            let (ok, detail) = ensure(on);
            let _ = tx.send((ok, on, detail));
        });
    rx
}

/// Test helper: parse `uhubctl` / script status lines.
pub fn parse_status_on(line: &str) -> Option<bool> {
    let l = line.to_ascii_lowercase();
    if l.contains("wifi-usb: on") {
        return Some(true);
    }
    if l.contains("wifi-usb: off") {
        return Some(false);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loc_1_1_3_is_hub_port() {
        assert_eq!(
            parse_sysfs_loc("1-1.3"),
            Some(("1-1".into(), 3))
        );
    }

    #[test]
    fn loc_nested_uses_last_dot() {
        assert_eq!(
            parse_sysfs_loc("1-1.2.1"),
            Some(("1-1.2".into(), 1))
        );
    }

    #[test]
    fn interface_names_are_ignored() {
        assert!(parse_sysfs_loc("1-1:1.0").is_none());
    }

    #[test]
    fn wanted_only_for_wifi_or_update() {
        assert!(!usb_wanted(false, false, false, false, false));
        assert!(usb_wanted(true, false, false, false, false));
        assert!(usb_wanted(false, false, true, false, false));
        assert!(usb_wanted(false, false, false, true, false));
        assert!(usb_wanted(false, true, false, false, false));
    }

    #[test]
    fn ensure_is_dry_in_unit_tests() {
        let (ok, detail) = ensure(true);
        assert!(ok);
        assert!(detail.contains("dry-run"));
    }
}
