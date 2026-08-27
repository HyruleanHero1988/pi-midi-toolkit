//! Appliance host hooks: Map/Thru (`midi-engine`), WIFI (`nmcli`), UPDATE (git/updater).

#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::process::Command;

fn run_capture(cmd: &mut Command, timeout_hint_secs: u64) -> (i32, String, String) {
    let _ = timeout_hint_secs;
    match cmd.output() {
        Ok(out) => {
            let code = out.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            (code, stdout, stderr)
        }
        Err(e) => (127, String::new(), e.to_string()),
    }
}

fn truncate_lines(text: &str, max_lines: usize, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        if i >= max_lines {
            out.push_str("…\n");
            break;
        }
        if out.len() + line.len() > max_chars {
            out.push_str("…\n");
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn find_repo_root() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_REPO_ROOT") {
        return PathBuf::from(p);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for anc in cwd.ancestors() {
        if anc.join(".git").exists()
            || (anc.join("Cargo.toml").exists() && anc.join("crates").is_dir())
        {
            return anc.to_path_buf();
        }
    }
    cwd
}

fn midi_engine_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("MIDI_ENGINE_BIN") {
        out.push(PathBuf::from(p));
    }
    let root = find_repo_root();
    for rel in [
        "bin/midi-engine",
        "dist/armv7/midi-engine",
        "target/release/midi-engine",
        "target/debug/midi-engine",
    ] {
        out.push(root.join(rel));
    }
    out.push(PathBuf::from("midi-engine"));
    out.push(PathBuf::from("bin/midi-engine"));
    out
}

#[cfg(target_os = "linux")]
fn resolve_midi_engine() -> Option<PathBuf> {
    for c in midi_engine_candidates() {
        if c.is_file() {
            return Some(c);
        }
    }
    let (code, out, _) = run_capture(Command::new("which").arg("midi-engine"), 3);
    if code == 0 && !out.is_empty() {
        let p = PathBuf::from(out.lines().next().unwrap_or(""));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
fn resolve_midi_engine() -> Option<PathBuf> {
    midi_engine_candidates().into_iter().find(|c| c.is_file())
}

pub fn map_status_line() -> String {
    #[cfg(target_os = "linux")]
    {
        if Path::new("/etc/systemd/system/midi-engine.service").exists()
            || Path::new("/lib/systemd/system/midi-engine.service").exists()
        {
            return "midi-engine unit present — prefer systemctl".into();
        }
        match resolve_midi_engine() {
            Some(p) => format!("midi-engine @ {}", p.display()),
            None => "midi-engine binary not found".into(),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        "Map/Thru runs on the Pi appliance (midi-engine)".into()
    }
}

pub fn map_list_ports() -> (String, Vec<String>) {
    #[cfg(not(target_os = "linux"))]
    {
        return (
            "Map runs on the Pi — use midi-engine list there".into(),
            vec!["Map/Thru is appliance-only on Linux (midi-engine).".into()],
        );
    }
    #[cfg(target_os = "linux")]
    {
        let Some(bin) = resolve_midi_engine() else {
            return (
                "midi-engine not found".into(),
                vec!["REFRESH: no midi-engine binary (bin/, dist/armv7/, PATH)".into()],
            );
        };
        let (code, stdout, stderr) = run_capture(Command::new(&bin).arg("list"), 8);
        let mut lines = Vec::new();
        if !stdout.is_empty() {
            for line in stdout.lines().take(24) {
                lines.push(line.to_string());
            }
        }
        if lines.is_empty() && !stderr.is_empty() {
            lines.push(truncate_lines(&stderr, 8, 400));
        }
        if lines.is_empty() {
            lines.push(format!("midi-engine list exit {code}"));
        }
        let status = if code == 0 {
            "ports listed — see LOG".into()
        } else {
            format!("list exit {code}")
        };
        (status, lines)
    }
}

pub fn map_thru_on() -> (String, Vec<String>) {
    #[cfg(not(target_os = "linux"))]
    {
        return (
            "Map runs on the Pi appliance".into(),
            vec!["THRU ON: start midi-engine via systemd on the Pi.".into()],
        );
    }
    #[cfg(target_os = "linux")]
    {
        let mut lines = Vec::new();
        if Path::new("/etc/systemd/system/midi-engine.service").exists()
            || Path::new("/lib/systemd/system/midi-engine.service").exists()
        {
            let msg = "start midi-engine via systemd: sudo systemctl start midi-engine";
            lines.push(msg.into());
            return ("use systemctl for thru".into(), lines);
        }
        let Some(bin) = resolve_midi_engine() else {
            return (
                "midi-engine not found".into(),
                vec!["THRU ON: no midi-engine binary".into()],
            );
        };
        match Command::new(&bin)
            .arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                let pid = child.id();
                // Detach: leak Child so process keeps running (appliance helper).
                std::mem::forget(child);
                lines.push(format!("spawned {} run (pid {pid})", bin.display()));
                (format!("thru started pid {pid}"), lines)
            }
            Err(e) => {
                lines.push(format!("spawn failed: {e}"));
                ("thru start failed".into(), lines)
            }
        }
    }
}

pub fn map_thru_off() -> (String, Vec<String>) {
    #[cfg(not(target_os = "linux"))]
    {
        return (
            "Map runs on the Pi appliance".into(),
            vec!["THRU OFF: stop midi-engine on the Pi (systemctl stop).".into()],
        );
    }
    #[cfg(target_os = "linux")]
    {
        let mut lines = Vec::new();
        if Path::new("/etc/systemd/system/midi-engine.service").exists()
            || Path::new("/lib/systemd/system/midi-engine.service").exists()
        {
            lines.push("stop midi-engine via systemd: sudo systemctl stop midi-engine".into());
            return ("use systemctl to stop thru".into(), lines);
        }
        let (code, stdout, stderr) = run_capture(
            Command::new("pkill").args(["-x", "midi-engine"]),
            5,
        );
        if !stdout.is_empty() {
            lines.push(stdout);
        }
        if !stderr.is_empty() {
            lines.push(stderr);
        }
        lines.push(format!("pkill midi-engine exit {code}"));
        (
            if code == 0 {
                "thru stop signaled".into()
            } else {
                format!("pkill exit {code}")
            },
            lines,
        )
    }
}

fn load_wifi_credentials() -> (Option<String>, Option<String>) {
    let mut ssid = std::env::var("WIFI_SSID").ok().filter(|s| !s.is_empty());
    let mut pass = std::env::var("WIFI_PASSWORD").ok().filter(|s| !s.is_empty());
    for path in [
        PathBuf::from(".wifi-credentials"),
        find_repo_root().join(".wifi-credentials"),
        find_repo_root().join("apps/pidi/.wifi-credentials"),
    ] {
        if !path.is_file() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || !line.contains('=') {
                    continue;
                }
                let (k, v) = line.split_once('=').unwrap();
                match k.trim() {
                    "WIFI_SSID" if ssid.is_none() => ssid = Some(v.trim().to_string()),
                    "WIFI_PASSWORD" if pass.is_none() => pass = Some(v.trim().to_string()),
                    _ => {}
                }
            }
        }
    }
    (ssid, pass)
}

pub fn wifi_action() -> (String, Vec<String>) {
    #[cfg(not(target_os = "linux"))]
    {
        let (ssid, _) = load_wifi_credentials();
        let mut lines = vec!["WIFI: nmcli runs on the Pi appliance.".into()];
        if let Some(s) = ssid {
            lines.push(format!("credentials present for SSID={s}"));
        } else {
            lines.push("no .wifi-credentials / WIFI_SSID on this host".into());
        }
        return ("WIFI — appliance (nmcli)".into(), lines);
    }
    #[cfg(target_os = "linux")]
    {
        let mut lines = Vec::new();
        // Prefer a fast device query — `device wifi list` rescans and can stall the UI.
        let (code, stdout, stderr) = run_capture(
            Command::new("nmcli").args(["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"]),
            8,
        );
        if code == 127 {
            lines.push("nmcli not installed".into());
            return ("WIFI missing".into(), lines);
        }
        if !stdout.is_empty() {
            for line in stdout.lines().take(8) {
                lines.push(line.to_string());
            }
        } else if !stderr.is_empty() {
            lines.push(truncate_lines(&stderr, 6, 300));
        }

        let wifi_dev = stdout.lines().find(|l| {
            let parts: Vec<_> = l.split(':').collect();
            parts.len() >= 3 && parts[1] == "wifi"
        });
        let (wifi_state, wifi_conn) = wifi_dev
            .map(|l| {
                let parts: Vec<_> = l.split(':').collect();
                (
                    parts.get(2).copied().unwrap_or(""),
                    parts.get(3).copied().unwrap_or(""),
                )
            })
            .unwrap_or(("", ""));

        if wifi_state.starts_with("connected") && !wifi_conn.is_empty() {
            let ssid = wifi_connection_ssid(wifi_conn).unwrap_or_else(|| wifi_conn.to_string());
            lines.push(format!("already on {ssid}"));
            return (format!("WIFI OK {ssid}"), lines);
        }

        let (ssid, password) = load_wifi_credentials();
        if let (Some(ssid), Some(password)) = (ssid, password) {
            lines.push(format!("CONNECT from creds → {ssid}"));
            let (cc, cout, cerr) = run_capture(
                Command::new("nmcli").args([
                    "device",
                    "wifi",
                    "connect",
                    &ssid,
                    "password",
                    &password,
                ]),
                45,
            );
            if !cout.is_empty() {
                lines.push(truncate_lines(&cout, 4, 200));
            }
            if !cerr.is_empty() {
                lines.push(truncate_lines(&cerr, 4, 200));
            }
            let status = if cc == 0 {
                format!("WIFI OK {ssid}")
            } else {
                format!("WIFI fail {cc}")
            };
            return (status, lines);
        }

        lines.push("no .wifi-credentials (WIFI_SSID / WIFI_PASSWORD)".into());
        if wifi_state.is_empty() {
            ("WIFI offline".into(), lines)
        } else {
            (format!("WIFI {wifi_state}"), lines)
        }
    }
}

#[cfg(target_os = "linux")]
fn wifi_connection_ssid(connection: &str) -> Option<String> {
    if connection.is_empty() {
        return None;
    }
    let (code, stdout, _) = run_capture(
        Command::new("nmcli").args([
            "-g",
            "802-11-wireless.ssid",
            "connection",
            "show",
            connection,
        ]),
        5,
    );
    if code == 0 {
        let ssid = stdout.trim();
        if !ssid.is_empty() {
            return Some(ssid.to_string());
        }
    }
    None
}

pub fn update_check() -> (String, Vec<String>) {
    let root = find_repo_root();
    let mut lines = Vec::new();

    let updater = root.join("apps/pidi/pidi/updater.py");
    if updater.is_file() {
        let (code, stdout, stderr) = run_capture(
            Command::new("python3")
                .arg(&updater)
                .arg("--check")
                .current_dir(&root),
            30,
        );
        if !stdout.is_empty() {
            lines.push(truncate_lines(&stdout, 12, 600));
        }
        if !stderr.is_empty() {
            lines.push(truncate_lines(&stderr, 6, 300));
        }
        let status = if code == 0 {
            "UPDATE check ok — see LOG".into()
        } else {
            format!("updater --check exit {code}")
        };
        return (status, lines);
    }

    // Fallback: git fetch + status
    let (fc, _, ferr) = run_capture(
        Command::new("git")
            .args(["fetch", "--quiet"])
            .current_dir(&root),
        45,
    );
    if fc != 0 && !ferr.is_empty() {
        lines.push(truncate_lines(&ferr, 4, 200));
    }
    let (sc, sout, serr) = run_capture(
        Command::new("git")
            .args(["status", "-sb"])
            .current_dir(&root),
        8,
    );
    if !sout.is_empty() {
        lines.push(sout.clone());
    }
    if !serr.is_empty() {
        lines.push(serr);
    }

    let (hc, head, _) = run_capture(
        Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&root),
        5,
    );
    let (uc, upstream, _) = run_capture(
        Command::new("git")
            .args(["rev-parse", "--short", "@{u}"])
            .current_dir(&root),
        5,
    );
    if hc == 0 && uc == 0 {
        lines.push(format!("HEAD {head}  upstream {upstream}"));
        let status = if head == upstream {
            format!("up to date ({head})")
        } else {
            format!("diverged/behind? {head} vs {upstream}")
        };
        return (status, lines);
    }
    let status = if sc == 0 {
        "git status — see LOG".into()
    } else {
        "UPDATE: no updater.py / git upstream".into()
    };
    (status, lines)
}

/// Soft reboot/poweroff via midi-tone `pi-power.sh` when present (Tk POWER menu).
pub fn pi_power(action: &str) -> (String, Vec<String>) {
    let action = if action == "reboot" { "reboot" } else { "poweroff" };
    #[cfg(not(target_os = "linux"))]
    {
        return (
            format!("POWER — {action} (appliance only)"),
            vec![format!("POWER runs pi-power.sh on the Pi ({action}).")],
        );
    }
    #[cfg(target_os = "linux")]
    {
        let candidates = [
            PathBuf::from("/home/ray/midi-tone/scripts/session/pi-power.sh"),
            find_repo_root().join("apps/pidi/scripts/session/pi-power.sh"),
            PathBuf::from("apps/pidi/scripts/session/pi-power.sh"),
        ];
        let Some(script) = candidates.into_iter().find(|p| p.is_file()) else {
            return (
                format!("POWER: pi-power.sh missing ({action})"),
                vec!["expected apps/pidi/scripts/session/pi-power.sh".into()],
            );
        };
        let script_arg = script.to_str().unwrap_or("pi-power.sh");
        let attempts: &[(&str, &[&str])] = &[
            ("pi-power.sh", &["sudo", "-n", script_arg, action]),
            ("systemctl", &["sudo", "-n", "systemctl", action]),
            (
                "bin",
                if action == "poweroff" {
                    &["sudo", "-n", "poweroff"]
                } else {
                    &["sudo", "-n", "reboot"]
                },
            ),
        ];
        let mut lines = Vec::new();
        for (label, args) in attempts {
            let (code, stdout, stderr) = run_capture(
                Command::new(args[0]).args(&args[1..]),
                25,
            );
            if code == 0 {
                return (
                    format!("POWER: {action}…"),
                    vec![format!("{label} ok")],
                );
            }
            lines.push(format!("{label} exit {code}"));
            if !stdout.is_empty() {
                lines.push(truncate_lines(&stdout, 2, 240));
            }
            if !stderr.is_empty() {
                lines.push(truncate_lines(&stderr, 2, 240));
            }
        }
        (
            format!("POWER: {action} failed (see LOG)"),
            lines,
        )
    }
}

/// Legacy direct poweroff (tests / callers that skip the menu).
pub fn power_action() -> (String, Vec<String>) {
    pi_power("poweroff")
}
