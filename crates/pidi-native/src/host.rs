//! Appliance host hooks: Map/Thru (`midi-engine`), WIFI (`nmcli`), UPDATE (git/updater).
//!
//! Subprocesses must never run on the UI/touch thread. Call [`HostTask::spawn`]
//! and poll the receiver from `tick`.

#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static DRY_RUN: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
pub fn set_dry_run(on: bool) {
    DRY_RUN.with(|c| c.set(on));
}

fn dry_run() -> bool {
    #[cfg(test)]
    {
        if DRY_RUN.with(|c| c.get()) {
            return true;
        }
    }
    false
}

/// Kill a hung helper so UPDATE/WIFI/MAP cannot freeze the kiosk.
fn kill_pid(pid: u32) {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
}

fn run_capture(cmd: &mut Command, timeout_secs: u64) -> (i32, String, String) {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (127, String::new(), e.to_string()),
    };
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let timeout = Duration::from_secs(timeout_secs.max(1));
    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) => {
            let code = out.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            (code, stdout, stderr)
        }
        Ok(Err(e)) => (127, String::new(), e.to_string()),
        Err(_) => {
            kill_pid(pid);
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if rx.try_recv().is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            (
                124,
                String::new(),
                format!("timed out after {timeout_secs}s"),
            )
        }
    }
}

/// Appliance work that must not block the 60 Hz loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostTask {
    UpdateCheck,
    Wifi,
    MapList,
    MapThruOn,
    MapThruOff,
}

impl HostTask {
    pub fn busy_status(self) -> &'static str {
        match self {
            Self::UpdateCheck => "UPDATE checking",
            Self::Wifi => "WIFI working",
            Self::MapList => "MAP listing",
            Self::MapThruOn => "THRU starting",
            Self::MapThruOff => "THRU stopping",
        }
    }

    pub fn run(self) -> (String, Vec<String>) {
        if dry_run() {
            return (format!("{} dry-run", self.busy_status()), vec![]);
        }
        match self {
            Self::UpdateCheck => update_check(),
            Self::Wifi => wifi_action(),
            Self::MapList => map_list_ports(),
            Self::MapThruOn => map_thru_on(),
            Self::MapThruOff => map_thru_off(),
        }
    }

    /// Start the task on a worker thread. The UI polls [`Receiver::try_recv`].
    pub fn spawn(self) -> Receiver<(String, Vec<String>)> {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("pidi-host".into())
            .spawn(move || {
                let _ = tx.send(self.run());
            });
        rx
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

fn load_wifi_credentials() -> (Option<String>, Option<String>, String) {
    let mut ssid = std::env::var("WIFI_SSID").ok().filter(|s| !s.is_empty());
    let mut pass = std::env::var("WIFI_PASSWORD").ok();
    let mut iface = std::env::var("WIFI_IFACE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::wifi::DEFAULT_IFACE.to_string());
    let mut paths = vec![crate::paths::wifi_credentials_path()];
    paths.push(PathBuf::from(".wifi-credentials"));
    paths.push(find_repo_root().join(".wifi-credentials"));
    paths.push(find_repo_root().join("apps/pidi/.wifi-credentials"));
    for path in paths {
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
                    "WIFI_PASSWORD" if pass.is_none() => pass = Some(v.to_string()),
                    "WIFI_IFACE" if iface == crate::wifi::DEFAULT_IFACE => {
                        let t = v.trim();
                        if !t.is_empty() {
                            iface = t.to_string();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    (ssid, pass, iface)
}

#[cfg(target_os = "linux")]
fn nmcli(args: &[&str], timeout_hint_secs: u64) -> (i32, String, String) {
    run_capture(Command::new("nmcli").args(args), timeout_hint_secs)
}

#[cfg(target_os = "linux")]
fn nmcli_sudo_then(args: &[&str], timeout_hint_secs: u64) -> (i32, String, String) {
    let mut cmd = Command::new("sudo");
    cmd.arg("-n").arg("nmcli").args(args);
    let (code, out, err) = run_capture(&mut cmd, timeout_hint_secs);
    if code == 0 {
        return (code, out, err);
    }
    nmcli(args, timeout_hint_secs)
}

/// One-shot REJOIN from saved credentials (also used by the WIFI panel).
pub fn wifi_action() -> (String, Vec<String>) {
    let (ok, detail, lines) = wifi_rejoin();
    let status = if ok {
        format!("WIFI OK {detail}")
    } else {
        format!("WIFI {detail}")
    };
    (status, lines)
}

pub fn wifi_rejoin() -> (bool, String, Vec<String>) {
    #[cfg(not(target_os = "linux"))]
    {
        let (ssid, _, _) = load_wifi_credentials();
        let mut lines = vec!["WIFI: nmcli runs on the Pi appliance.".into()];
        if let Some(s) = ssid {
            lines.push(format!("credentials present for SSID={s}"));
        } else {
            lines.push("no .wifi-credentials / WIFI_SSID on this host".into());
        }
        return (false, "appliance (nmcli)".into(), lines);
    }
    #[cfg(target_os = "linux")]
    {
        wifi_rejoin_linux()
    }
}

#[cfg(target_os = "linux")]
fn wifi_rejoin_linux() -> (bool, String, Vec<String>) {
    let mut lines = Vec::new();
    let (code, stdout, stderr) = nmcli(
        &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"],
        8,
    );
    if code == 127 {
        lines.push("nmcli not installed".into());
        return (false, "missing".into(), lines);
    }
    if !stdout.is_empty() {
        for line in stdout.lines().take(6) {
            lines.push(line.to_string());
        }
    } else if !stderr.is_empty() {
        lines.push(truncate_lines(&stderr, 4, 200));
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
        return (true, ssid, lines);
    }

    let _ = nmcli_sudo_then(&["radio", "wifi", "on"], 8);

    let (ssid, password, iface) = load_wifi_credentials();
    if let Some(ssid) = ssid {
        let pass = password.unwrap_or_default();
        lines.push(format!("CONNECT from creds → {ssid}"));
        let (ok, detail) = wifi_connect(&ssid, &pass, &iface, false);
        lines.push(detail.clone());
        return (ok, detail, lines);
    }

    let (cc, cout, _) = nmcli(&["-t", "-f", "NAME,TYPE", "connection", "show"], 8);
    let mut wifi_names = Vec::new();
    if cc == 0 {
        for line in cout.lines() {
            if line.contains(":802-11-wireless") || line.ends_with(":wifi") {
                if let Some(name) = line.split(':').next() {
                    if !name.is_empty() {
                        wifi_names.push(name.to_string());
                    }
                }
            }
        }
    }
    wifi_names.sort_by_key(|n| if n == "preconfigured" { 0 } else { 1 });
    for name in &wifi_names {
        let (code, out, err) = nmcli_sudo_then(&["connection", "up", name], 45);
        if code == 0 {
            lines.push(format!("up {name}"));
            let detail = if out.trim().is_empty() {
                format!("up {name}")
            } else {
                out
            };
            return (true, detail, lines);
        }
        if !err.is_empty() {
            lines.push(truncate_lines(&err, 2, 120));
        }
    }

    let iface = iface_or_default();
    let (code, out, err) = nmcli_sudo_then(&["device", "connect", &iface], 45);
    if code == 0 {
        lines.push(format!("connect {iface}"));
        let detail = if out.trim().is_empty() {
            "connected".into()
        } else {
            out
        };
        return (true, detail, lines);
    }

    lines.push("no .wifi-credentials — pick a network on this screen".into());
    if !err.is_empty() {
        lines.push(truncate_lines(&err, 3, 160));
    }
    if wifi_state.is_empty() {
        (false, "offline".into(), lines)
    } else {
        (false, wifi_state.to_string(), lines)
    }
}

fn iface_or_default() -> String {
    let (_, _, iface) = load_wifi_credentials();
    iface
}

pub fn wifi_scan(rescan: bool) -> (Vec<crate::wifi::WifiNetwork>, String) {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = rescan;
        return (
            Vec::new(),
            "Wi-Fi scan runs on the Pi appliance (nmcli)".into(),
        );
    }
    #[cfg(target_os = "linux")]
    {
        let (code, _, _) = nmcli(&["-t", "-f", "WIFI", "radio"], 3);
        if code == 127 {
            return (Vec::new(), "nmcli not installed on this box".into());
        }
        let _ = nmcli_sudo_then(&["radio", "wifi", "on"], 8);
        let iface = iface_or_default();
        if rescan {
            let (c, _, _) =
                nmcli_sudo_then(&["device", "wifi", "rescan", "ifname", &iface], 20);
            if c != 0 {
                let _ = nmcli(&["device", "wifi", "rescan"], 20);
            }
        }
        let (code, out, err) = nmcli(
            &[
                "-t",
                "-f",
                "IN-USE,SSID,SIGNAL,SECURITY",
                "device",
                "wifi",
                "list",
            ],
            15,
        );
        if code != 0 {
            let msg = if !err.trim().is_empty() {
                err
            } else if !out.trim().is_empty() {
                out
            } else {
                "Wi-Fi scan failed".into()
            };
            return (Vec::new(), msg);
        }
        let networks = crate::wifi::parse_wifi_list(&out);
        if networks.is_empty() {
            (
                networks,
                "No networks found — move closer or tap SCAN again".into(),
            )
        } else {
            (networks, String::new())
        }
    }
}

pub fn wifi_connect(ssid: &str, password: &str, iface: &str, remember: bool) -> (bool, String) {
    let ssid = ssid.trim();
    if ssid.is_empty() {
        return (false, "No network selected".into());
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (password, iface, remember);
        return (false, "Wi-Fi join runs on the Pi appliance (nmcli)".into());
    }
    #[cfg(target_os = "linux")]
    {
        let (code, _, _) = nmcli(&["-t", "-f", "WIFI", "radio"], 3);
        if code == 127 {
            return (false, "nmcli not installed on this box".into());
        }
        let _ = nmcli_sudo_then(&["radio", "wifi", "on"], 8);
        let iface = if iface.is_empty() {
            crate::wifi::DEFAULT_IFACE
        } else {
            iface
        };
        let mut args = vec![
            "device".into(),
            "wifi".into(),
            "connect".into(),
            ssid.to_string(),
            "ifname".into(),
            iface.to_string(),
        ];
        if !password.is_empty() {
            args.push("password".into());
            args.push(password.to_string());
        }
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let (code, out, err) = nmcli_sudo_then(&arg_refs, 60);
        if code != 0 {
            let msg = if !err.trim().is_empty() {
                err
            } else if !out.trim().is_empty() {
                out
            } else {
                format!("failed to join {ssid}")
            };
            return (false, msg);
        }
        if remember {
            let path = crate::paths::wifi_credentials_path();
            if let Err(e) = crate::wifi::save_wifi_credentials(&path, ssid, password, iface) {
                return (true, format!("joined {ssid} (save creds failed: {e})"));
            }
        }
        (true, format!("joined {ssid}"))
    }
}

#[cfg(target_os = "linux")]
fn wifi_connection_ssid(connection: &str) -> Option<String> {
    if connection.is_empty() {
        return None;
    }
    let (code, stdout, _) = nmcli(
        &["-g", "802-11-wireless.ssid", "connection", "show", connection],
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

#[derive(Debug, Clone)]
pub enum WifiJobResult {
    Scan {
        networks: Vec<crate::wifi::WifiNetwork>,
        error: String,
    },
    Rejoin {
        ok: bool,
        detail: String,
        lines: Vec<String>,
    },
    Join {
        ok: bool,
        detail: String,
        ssid: String,
    },
}

#[derive(Debug, Clone)]
pub enum WifiJobKind {
    Scan,
    Rejoin,
    Join { ssid: String, password: String },
}

pub fn spawn_wifi_job(kind: WifiJobKind) -> std::sync::mpsc::Receiver<WifiJobResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    let name = match &kind {
        WifiJobKind::Scan => "pidi-wifi-scan",
        WifiJobKind::Rejoin => "pidi-wifi-rejoin",
        WifiJobKind::Join { .. } => "pidi-wifi-join",
    };
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let result = match kind {
                WifiJobKind::Scan => {
                    let (networks, error) = wifi_scan(true);
                    WifiJobResult::Scan { networks, error }
                }
                WifiJobKind::Rejoin => {
                    let (ok, detail, lines) = wifi_rejoin();
                    WifiJobResult::Rejoin { ok, detail, lines }
                }
                WifiJobKind::Join { ssid, password } => {
                    let iface = iface_or_default();
                    let (ok, detail) = wifi_connect(&ssid, &password, &iface, true);
                    WifiJobResult::Join { ok, detail, ssid }
                }
            };
            let _ = tx.send(result);
        })
        .expect("spawn wifi job");
    rx
}

pub fn update_check() -> (String, Vec<String>) {
    let result = update_check_detailed();
    (result.status, result.lines)
}

#[derive(Debug, Clone)]
pub struct UpdateCheckResult {
    pub status: String,
    pub lines: Vec<String>,
    pub available: bool,
    pub ok: bool,
    /// Staged `pidi-native` was installed — caller should re-exec this process.
    pub reload_kiosk: bool,
}

/// Fast local stamp for the Update panel — file reads only (no network).
pub fn update_local_status() -> String {
    let root = find_repo_root();
    let mut parts = Vec::new();

    for candidate in [
        root.join("apps/pidi/version.json"),
        root.join("version.json"),
        PathBuf::from("/home/ray/midi-tone/version.json"),
        PathBuf::from("/home/ray/pi-midi-toolkit/apps/pidi/version.json"),
    ] {
        if let Some(line) = read_version_json_line(&candidate) {
            parts.push(line);
            break;
        }
    }

    let ver = root.join("dist/armv7/VERSION");
    if let Ok(text) = std::fs::read_to_string(&ver) {
        let mut sha = None;
        let mut glibc = None;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("git_sha=") {
                sha = Some(rest.trim().chars().take(7).collect::<String>());
            }
            if let Some(rest) = line.strip_prefix("host_glibc=") {
                glibc = Some(rest.trim().to_string());
            }
        }
        if let Some(s) = sha {
            parts.push(format!("engines {s}"));
        }
        if let Some(g) = glibc {
            parts.push(format!("glibc {g}"));
        }
    }

    if parts.is_empty() {
        "Running: unknown — tap CHECK for GitHub".into()
    } else {
        format!("Running: {}", parts.join(" · "))
    }
}

fn read_version_json_line(path: &PathBuf) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let sha = text
        .lines()
        .find_map(|l| {
            let t = l.trim().trim_matches(',');
            t.strip_prefix("\"sha\"")
                .or_else(|| t.strip_prefix("\"sha\":"))
                .map(|s| s.trim().trim_matches(':').trim().trim_matches('"').to_string())
                .filter(|s| !s.is_empty() && s != "null")
        })
        .or_else(|| {
            // crude: "sha": "abcdef..."
            let idx = text.find("\"sha\"")?;
            let after = &text[idx + 5..];
            let q1 = after.find('"')?;
            let rest = &after[q1 + 1..];
            let q2 = rest.find('"')?;
            Some(rest[..q2].to_string())
        })?;
    let short: String = sha.chars().take(7).collect();
    let branch = text.find("\"branch\"").and_then(|idx| {
        let after = &text[idx + 8..];
        let q1 = after.find('"')?;
        let rest = &after[q1 + 1..];
        let q2 = rest.find('"')?;
        Some(rest[..q2].to_string())
    });
    Some(match branch {
        Some(b) if !b.is_empty() => format!("{short} ({b})"),
        _ => short,
    })
}

pub fn update_check_detailed() -> UpdateCheckResult {
    let root = find_repo_root();
    let mut lines = Vec::new();

    let updater = root.join("apps/pidi/pidi/updater.py");
    if updater.is_file() {
        let (code, stdout, stderr) = run_capture(
            Command::new("python3")
                .arg(&updater)
                .arg("--check")
                .current_dir(&root),
            60,
        );
        if !stdout.is_empty() {
            lines.push(truncate_lines(&stdout, 12, 600));
        }
        if !stderr.is_empty() {
            lines.push(truncate_lines(&stderr, 6, 300));
        }
        let blob = format!("{stdout}\n{stderr}");
        let available = blob.to_ascii_lowercase().contains("update available")
            || blob.contains("No local version stamp");
        let status = if code == 0 {
            if available {
                "UPDATE available — tap INSTALL".into()
            } else if blob.to_ascii_lowercase().contains("already on") {
                "up to date".into()
            } else {
                stdout.lines().next().unwrap_or("CHECK ok").to_string()
            }
        } else {
            stdout
                .lines()
                .next()
                .or_else(|| stderr.lines().next())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("CHECK failed (exit {code})"))
        };
        return UpdateCheckResult {
            status,
            lines,
            available: code == 0 && available,
            ok: code == 0,
            reload_kiosk: false,
        };
    }

    // Fallback: git fetch + compare HEAD to upstream (still network — caller must be async).
    let (fc, _, ferr) = run_capture(
        Command::new("git")
            .args(["fetch", "--quiet"])
            .current_dir(&root),
        45,
    );
    if fc != 0 && !ferr.is_empty() {
        lines.push(truncate_lines(&ferr, 4, 200));
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
        let available = head != upstream;
        let status = if available {
            format!("behind? {head} vs {upstream}")
        } else {
            format!("up to date ({head})")
        };
        return UpdateCheckResult {
            status,
            lines,
            available,
            ok: true,
            reload_kiosk: false,
        };
    }
    UpdateCheckResult {
        status: "UPDATE: no updater.py / git upstream".into(),
        lines,
        available: false,
        ok: false,
        reload_kiosk: false,
    }
}

pub fn update_apply() -> UpdateCheckResult {
    let root = find_repo_root();
    let mut lines = Vec::new();
    let updater = root.join("apps/pidi/pidi/updater.py");
    if !updater.is_file() {
        return UpdateCheckResult {
            status: "UPDATE: updater.py missing".into(),
            lines,
            available: false,
            ok: false,
            reload_kiosk: false,
        };
    }
    let (code, stdout, stderr) = run_capture(
        Command::new("python3")
            .arg(&updater)
            .arg("--apply")
            .current_dir(&root),
        600,
    );
    update_apply_result(code, &stdout, &stderr)
}

pub(crate) fn update_apply_result(code: i32, stdout: &str, stderr: &str) -> UpdateCheckResult {
    let mut lines = Vec::new();
    if !stdout.is_empty() {
        lines.push(truncate_lines(stdout, 20, 1200));
    }
    if !stderr.is_empty() {
        lines.push(truncate_lines(stderr, 8, 400));
    }
    let blob = format!("{stdout}\n{stderr}");
    let reload_kiosk = code == 0
        && (blob.contains("RELOAD_KIOSK=1") || blob.contains("Installed pidi-native"));
    let status = if code == 0 {
        let last = stdout
            .lines()
            .rev()
            .find(|l| !l.contains("RELOAD_KIOSK="))
            .unwrap_or("install finished");
        if reload_kiosk {
            format!("{last}\nReloading kiosk…")
        } else {
            last.to_string()
        }
    } else {
        format!(
            "INSTALL failed (exit {code}): {}",
            stderr
                .lines()
                .next()
                .or_else(|| stdout.lines().next())
                .unwrap_or("see LOG")
        )
    };
    UpdateCheckResult {
        status,
        lines,
        available: false,
        ok: code == 0,
        reload_kiosk,
    }
}

/// Replace this process with the binary now on disk (same argv / PID).
///
/// Used after OTA installs a new `pidi-native` so we never `systemctl stop`
/// the kiosk from inside the update. Callers must drop the SDL/KMSDRM
/// presenter first so the new image can acquire the display.
pub fn reexec_current_process() -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        let err = Command::new(&exe).args(args).exec();
        Err(format!("reexec {} failed: {err}", exe.display()))
    }
    #[cfg(not(unix))]
    {
        Err("reexec is Unix-only".into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateJobKind {
    Check,
    Apply,
}

/// Spawn CHECK or INSTALL off the UI thread. Poll with `try_recv`.
pub fn spawn_update_job(
    kind: UpdateJobKind,
) -> std::sync::mpsc::Receiver<UpdateCheckResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name(match kind {
            UpdateJobKind::Check => "pidi-update-check".into(),
            UpdateJobKind::Apply => "pidi-update-apply".into(),
        })
        .spawn(move || {
            let result = match kind {
                UpdateJobKind::Check => update_check_detailed(),
                UpdateJobKind::Apply => update_apply(),
            };
            let _ = tx.send(result);
        })
        .expect("spawn update job");
    rx
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_capture_kills_a_hung_command() {
        let start = Instant::now();
        let (code, _, err) = run_capture(Command::new("sleep").arg("20"), 1);
        assert_eq!(code, 124, "err={err}");
        assert!(err.contains("timed out"));
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "timeout took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn dry_run_skips_subprocesses() {
        set_dry_run(true);
        let (status, _) = HostTask::UpdateCheck.run();
        set_dry_run(false);
        assert!(status.contains("dry-run"), "{status}");
    }

    #[test]
    fn apply_result_reloads_kiosk_when_native_bin_installed() {
        let result = update_apply_result(
            0,
            "[90% · 0:12] Installed pidi-native → bin/\nRELOAD_KIOSK=1\nnow abc1234 (master)",
            "",
        );
        assert!(result.ok);
        assert!(result.reload_kiosk);
        assert!(result.status.contains("Reloading kiosk"));
        assert!(!result.status.contains("RELOAD_KIOSK"));
    }

    #[test]
    fn apply_result_keeps_kiosk_up_when_only_engines_match() {
        let result = update_apply_result(0, "Already on latest.\nnow abc1234 (master)", "");
        assert!(result.ok);
        assert!(!result.reload_kiosk);
        assert!(!result.status.contains("Reloading"));
    }

    #[test]
    fn apply_result_failed_does_not_reload() {
        let result = update_apply_result(1, "", "INSTALL failed (exit 1): network error");
        assert!(!result.ok);
        assert!(!result.reload_kiosk);
        assert!(result.status.contains("INSTALL failed"));
    }
}
