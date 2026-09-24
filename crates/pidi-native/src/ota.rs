//! In-process SET→UPDATE: GitHub CHECK/INSTALL with no Python helper.
//!
//! Overlay only [`PIDI_REPO_ROOT`]. User data stays under [`crate::paths::data_root`].
//! Live `bin/` is not copied from the archive; staged `dist/armv7` engines are
//! installed with an atomic rename. The kiosk process is never stopped.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::read::GzDecoder;
use serde::Deserialize;
use tar::Archive;

use crate::host::UpdateCheckResult;
use crate::paths;

const DEFAULT_OWNER: &str = "HyruleanHero1988";
const DEFAULT_REPO: &str = "pi-midi-toolkit";
const DEFAULT_BRANCH: &str = "master";
const USER_AGENT: &str = "pidi-native/1.0";
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

const ENGINE_BINS: &[&str] = &["midi-engine", "jambox-engine", "pidi-native"];
const STOPPABLE_UNITS: &[&str] = &["jambox-engine", "midi-engine"];

const SKIP_PREFIXES: &[&str] = &[
    "bin",
    "target",
    ".git",
    ".venv",
    ".cursor",
    "presets/active.json",
    "apps/pidi/settings.json",
    "apps/pidi/songs",
    "apps/pidi/phrases",
    "apps/pidi/user-presets",
    "apps/pidi/user-wavetables",
    "apps/pidi/.pi-credentials",
    "apps/pidi/.update-credentials",
    "apps/pidi/.wifi-credentials",
    "apps/pidi/.venv",
];

#[derive(Debug, Clone, Default, Deserialize)]
struct VersionStamp {
    #[serde(default)]
    sha: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    source: String,
}

#[derive(Debug, Deserialize)]
struct GithubCommit {
    sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub installed_bins: Vec<String>,
    pub reload_kiosk: bool,
    pub lines: Vec<String>,
}

pub fn repo_root() -> PathBuf {
    if let Ok(p) = std::env::var("PIDI_REPO_ROOT") {
        let p = p.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for anc in cwd.ancestors() {
        if looks_like_repo_root(anc) || anc.join(".git").exists() {
            return anc.to_path_buf();
        }
    }
    cwd
}

pub fn looks_like_repo_root(path: &Path) -> bool {
    if !path.join("Cargo.toml").is_file() {
        return false;
    }
    path.join("crates").join("pidi-native").is_dir()
        || path.join("dist").join("armv7").join("pidi-native").is_file()
        || path.join("dist").join("armv7").join("jambox-engine").is_file()
}

pub fn find_extracted_root(extract_dir: &Path) -> Result<PathBuf, String> {
    if looks_like_repo_root(extract_dir) {
        return Ok(extract_dir.to_path_buf());
    }
    let mut found = None;
    let entries = fs::read_dir(extract_dir)
        .map_err(|e| format!("cannot read extract dir: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && looks_like_repo_root(&path) {
            if found.is_some() {
                return Err("downloaded archive was not a full pi-midi-toolkit tree".into());
            }
            found = Some(path);
        }
    }
    found.ok_or_else(|| "downloaded archive was not a full pi-midi-toolkit tree".into())
}

pub fn should_skip_overlay(rel: &Path) -> bool {
    let posix = rel.to_string_lossy().replace('\\', "/");
    if posix.is_empty() {
        return true;
    }
    SKIP_PREFIXES.iter().any(|skip| posix == *skip || posix.starts_with(&format!("{skip}/")))
}

pub fn overlay_tree(src: &Path, dest: &Path, data_root: &Path) -> Result<Vec<String>, String> {
    let mut written = Vec::new();
    overlay_walk(src, dest, dest, data_root, Path::new(""), &mut written)?;
    Ok(written)
}

fn overlay_walk(
    src_root: &Path,
    dest_root: &Path,
    dest: &Path,
    data_root: &Path,
    rel: &Path,
    written: &mut Vec<String>,
) -> Result<(), String> {
    let src = if rel.as_os_str().is_empty() {
        src_root.to_path_buf()
    } else {
        src_root.join(rel)
    };
    if !rel.as_os_str().is_empty() && should_skip_overlay(rel) {
        return Ok(());
    }
    if is_under(dest, data_root) && dest != dest_root {
        return Ok(());
    }
    if src.is_dir() {
        if !rel.as_os_str().is_empty() {
            fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
        }
        for entry in fs::read_dir(&src).map_err(|e| format!("read {}: {e}", src.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let child_rel = rel.join(&name);
            overlay_walk(
                src_root,
                dest_root,
                &dest.join(name),
                data_root,
                &child_rel,
                written,
            )?;
        }
        return Ok(());
    }
    if src.is_file() {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        fs::copy(&src, dest).map_err(|e| format!("copy {}: {e}", dest.display()))?;
        written.push(rel.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}

fn is_under(path: &Path, root: &Path) -> bool {
    let Ok(path) = path.canonicalize().or_else(|_| Ok::<_, io::Error>(path.to_path_buf())) else {
        return false;
    };
    let Ok(root) = root.canonicalize().or_else(|_| Ok::<_, io::Error>(root.to_path_buf())) else {
        return false;
    };
    path.starts_with(root)
}

pub fn files_identical(a: &Path, b: &Path) -> bool {
    let Ok(ma) = fs::metadata(a) else {
        return false;
    };
    let Ok(mb) = fs::metadata(b) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    let Ok(ba) = fs::read(a) else {
        return false;
    };
    let Ok(bb) = fs::read(b) else {
        return false;
    };
    ba == bb
}

pub fn atomic_install(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = dest.with_file_name(format!(
        "{}.ota-new",
        dest.file_name().and_then(|s| s.to_str()).unwrap_or("bin")
    ));
    let _ = fs::remove_file(&tmp);
    fs::copy(src, &tmp).map_err(|e| format!("stage {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(&tmp)
            .map_err(|e| e.to_string())?
            .permissions();
        perm.set_mode(perm.mode() | 0o111);
        fs::set_permissions(&tmp, perm).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, dest).map_err(|e| format!("replace {}: {e}", dest.display()))?;
    Ok(())
}

pub fn install_staged_bins(repo: &Path) -> Result<Vec<String>, String> {
    let src_dir = repo.join("dist").join("armv7");
    let dest_dir = repo.join("bin");
    if !src_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut installed = Vec::new();
    for name in ENGINE_BINS {
        let src = src_dir.join(name);
        if !src.is_file() {
            continue;
        }
        let dest = dest_dir.join(name);
        if files_identical(&src, &dest) {
            continue;
        }
        atomic_install(&src, &dest)?;
        installed.push((*name).to_string());
    }
    Ok(installed)
}

/// Overlay extracted tree + install staged bins. No network, no systemctl.
pub fn apply_extracted(
    src_root: &Path,
    dest_root: &Path,
    data_root: &Path,
) -> Result<ApplyOutcome, String> {
    if !looks_like_repo_root(src_root) {
        return Err("downloaded archive was not a full pi-midi-toolkit tree".into());
    }
    let mut lines = Vec::new();
    lines.push("Installing full repo…".into());
    overlay_tree(src_root, dest_root, data_root)?;
    let installed = install_staged_bins(dest_root)?;
    for name in &installed {
        lines.push(format!("Installed {name} → bin/"));
    }
    let reload_kiosk = installed.iter().any(|n| n == "pidi-native");
    if reload_kiosk {
        lines.push("RELOAD_KIOSK=1".into());
    }
    Ok(ApplyOutcome {
        installed_bins: installed,
        reload_kiosk,
        lines,
    })
}

fn local_stamp() -> VersionStamp {
    for path in version_read_paths() {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(stamp) = serde_json::from_str::<VersionStamp>(&text) {
                if !stamp.sha.is_empty() {
                    return stamp;
                }
            }
        }
    }
    VersionStamp::default()
}

pub fn local_status_line() -> String {
    let mut parts = Vec::new();
    let stamp = local_stamp();
    if !stamp.sha.is_empty() {
        let short: String = stamp.sha.chars().take(7).collect();
        parts.push(if stamp.branch.is_empty() {
            short
        } else {
            format!("{short} ({})", stamp.branch)
        });
        if !stamp.source.is_empty() && stamp.source != "unknown" {
            parts.push(stamp.source);
        }
    }
    let ver = repo_root().join("dist/armv7/VERSION");
    if let Ok(text) = fs::read_to_string(ver) {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("git_sha=") {
                parts.push(format!(
                    "engines {}",
                    rest.trim().chars().take(7).collect::<String>()
                ));
            }
            if let Some(rest) = line.strip_prefix("host_glibc=") {
                parts.push(format!("glibc {}", rest.trim()));
            }
        }
    }
    if parts.is_empty() {
        "Running: unknown — tap CHECK for GitHub".into()
    } else {
        format!("Running: {}", parts.join(" · "))
    }
}

fn version_read_paths() -> Vec<PathBuf> {
    vec![
        paths::data_root().join("version.json"),
        repo_root().join("version.json"),
        repo_root().join("apps/pidi/version.json"),
    ]
}

fn write_version_stamp(sha: &str, branch: &str, source: &str) -> Result<(), String> {
    let body = serde_json::json!({
        "sha": sha,
        "branch": branch,
        "source": source,
        "repo_url": format!("https://github.com/{}/{}", owner_repo().0, owner_repo().1),
    });
    let text = serde_json::to_string_pretty(&body).map_err(|e| e.to_string())? + "\n";
    for path in [
        paths::data_root().join("version.json"),
        repo_root().join("version.json"),
    ] {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&path, &text).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn owner_repo() -> (String, String) {
    if let Ok(raw) = std::env::var("PIDI_OTA_REPO") {
        let raw = raw.trim();
        if let Some((o, r)) = raw.split_once('/') {
            if !o.is_empty() && !r.is_empty() {
                return (o.to_string(), r.trim_end_matches(".git").to_string());
            }
        }
    }
    (DEFAULT_OWNER.into(), DEFAULT_REPO.into())
}

fn http_agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .build()
}

fn remote_head(branch: &str) -> Result<String, String> {
    let (owner, repo) = owner_repo();
    let url = format!("https://api.github.com/repos/{owner}/{repo}/commits/{branch}");
    let agent = http_agent(CHECK_TIMEOUT);
    let resp = agent
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("network error: {e}"))?;
    if resp.status() == 401 || resp.status() == 403 || resp.status() == 404 {
        return Err(format!(
            "can't reach GitHub (HTTP {}) — check network / branch name",
            resp.status()
        ));
    }
    if resp.status() >= 300 {
        return Err(format!("GitHub API error {}", resp.status()));
    }
    let commit: GithubCommit = resp
        .into_json()
        .map_err(|e| format!("GitHub API returned non-JSON: {e}"))?;
    if commit.sha.is_empty() {
        return Err("GitHub API response had no commit SHA".into());
    }
    Ok(commit.sha)
}

fn download_archive(branch: &str, dest: &Path) -> Result<(), String> {
    let (owner, repo) = owner_repo();
    let url = format!("https://codeload.github.com/{owner}/{repo}/tar.gz/refs/heads/{branch}");
    let agent = http_agent(DOWNLOAD_TIMEOUT);
    let resp = agent
        .get(&url)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|e| format!("network error: {e}"))?;
    if resp.status() == 401 || resp.status() == 403 || resp.status() == 404 {
        return Err(format!(
            "download failed (HTTP {}) — check network / branch name",
            resp.status()
        ));
    }
    if resp.status() >= 300 {
        return Err(format!("download failed (HTTP {})", resp.status()));
    }
    let mut reader = resp.into_reader();
    let mut file = File::create(dest).map_err(|e| format!("temp archive: {e}"))?;
    io::copy(&mut reader, &mut file).map_err(|e| format!("download: {e}"))?;
    if dest.metadata().map(|m| m.len()).unwrap_or(0) < 64 {
        return Err("download was empty".into());
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let gz = GzDecoder::new(file);
    let mut tar = Archive::new(gz);
    tar.unpack(dest)
        .map_err(|e| format!("unpack failed: {e}"))?;
    Ok(())
}

fn restart_audio_engines(lines: &mut Vec<String>) {
    if !cfg!(target_os = "linux") {
        return;
    }
    for unit in STOPPABLE_UNITS {
        let enabled = std::process::Command::new("systemctl")
            .args(["is-enabled", unit])
            .output();
        let ok = enabled
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            continue;
        }
        lines.push(format!("Restarting {unit}…"));
        let _ = std::process::Command::new("sudo")
            .args(["-n", "systemctl", "restart", unit])
            .status();
    }
}

pub fn check() -> UpdateCheckResult {
    let _ = crate::wifi_power::ensure(true);
    let local = local_stamp();
    let branch = if local.branch.is_empty() {
        DEFAULT_BRANCH.to_string()
    } else {
        local.branch.clone()
    };
    match remote_head(&branch) {
        Ok(remote) => {
            let short_local: String = local.sha.chars().take(7).collect();
            let short_remote: String = remote.chars().take(7).collect();
            if local.sha.is_empty() {
                UpdateCheckResult {
                    status: format!("No local version stamp — remote {branch} is {short_remote}"),
                    lines: vec![format!("remote {remote}")],
                    available: true,
                    ok: true,
                    reload_kiosk: false,
                }
            } else if local.sha == remote {
                UpdateCheckResult {
                    status: format!("up to date"),
                    lines: vec![format!("Already on {branch} {short_local}")],
                    available: false,
                    ok: true,
                    reload_kiosk: false,
                }
            } else {
                UpdateCheckResult {
                    status: "UPDATE available — tap INSTALL".into(),
                    lines: vec![format!(
                        "Update available: {short_local} → {short_remote} ({branch})"
                    )],
                    available: true,
                    ok: true,
                    reload_kiosk: false,
                }
            }
        }
        Err(err) => UpdateCheckResult {
            status: err.clone(),
            lines: vec![err],
            available: false,
            ok: false,
            reload_kiosk: false,
        },
    }
}

pub fn apply() -> UpdateCheckResult {
    let _ = crate::wifi_power::ensure(true);
    let local = local_stamp();
    let branch = if local.branch.is_empty() {
        DEFAULT_BRANCH.to_string()
    } else {
        local.branch.clone()
    };
    let remote = match remote_head(&branch) {
        Ok(sha) => sha,
        Err(err) => {
            return UpdateCheckResult {
                status: format!("INSTALL failed: {err}"),
                lines: vec![err],
                available: false,
                ok: false,
                reload_kiosk: false,
            };
        }
    };
    if !local.sha.is_empty() && local.sha == remote {
        let _ = write_version_stamp(&remote, &branch, "archive");
        return UpdateCheckResult {
            status: format!("Already on latest.\nnow {} ({branch})", short_sha(&remote)),
            lines: vec!["Already on latest.".into()],
            available: false,
            ok: true,
            reload_kiosk: false,
        };
    }

    let tmp = std::env::temp_dir().join(format!("pidi-ota-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    if let Err(err) = fs::create_dir_all(&tmp) {
        return fail(&format!("temp dir: {err}"));
    }
    let tar_path = tmp.join("src.tar.gz");
    let extract_dir = tmp.join("src");
    let mut lines = vec!["Downloading latest code…".into()];
    if let Err(err) = download_archive(&branch, &tar_path) {
        let _ = fs::remove_dir_all(&tmp);
        return fail(&err);
    }
    lines.push("Unpacking…".into());
    if let Err(err) = extract_tar_gz(&tar_path, &extract_dir) {
        let _ = fs::remove_dir_all(&tmp);
        return fail(&err);
    }
    let src_root = match find_extracted_root(&extract_dir) {
        Ok(p) => p,
        Err(err) => {
            let _ = fs::remove_dir_all(&tmp);
            return fail(&err);
        }
    };
    let dest = repo_root();
    let data = paths::data_root();
    let outcome = match apply_extracted(&src_root, &dest, &data) {
        Ok(o) => o,
        Err(err) => {
            let _ = fs::remove_dir_all(&tmp);
            return fail(&err);
        }
    };
    lines.extend(outcome.lines.iter().cloned());
    if !outcome.installed_bins.is_empty() {
        restart_audio_engines(&mut lines);
    }
    if let Err(err) = write_version_stamp(&remote, &branch, "archive") {
        let _ = fs::remove_dir_all(&tmp);
        return fail(&err);
    }
    let _ = fs::remove_dir_all(&tmp);
    let status = format!(
        "Installed {branch} {} ({})\nnow {} ({branch})",
        short_sha(&remote),
        if outcome.installed_bins.is_empty() {
            "tree only".into()
        } else {
            format!("updated {}", outcome.installed_bins.join("+"))
        },
        short_sha(&remote)
    );
    let status = if outcome.reload_kiosk {
        format!("{status}\nReloading kiosk...")
    } else {
        status
    };
    UpdateCheckResult {
        status,
        lines,
        available: false,
        ok: true,
        reload_kiosk: outcome.reload_kiosk,
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn fail(err: &str) -> UpdateCheckResult {
    UpdateCheckResult {
        status: format!("INSTALL failed: {err}"),
        lines: vec![err.to_string()],
        available: false,
        ok: false,
        reload_kiosk: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn uniq_dir(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pidi-ota-test-{}-{}-{}",
            tag,
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, body: &str) {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn native_tree(root: &Path) {
        write(&root.join("Cargo.toml"), "[workspace]\n");
        write(
            &root.join("crates/pidi-native/src/lib.rs"),
            "pub fn x() {}\n",
        );
    }

    #[test]
    fn native_tree_is_accepted() {
        let dir = uniq_dir("native");
        native_tree(&dir);
        assert!(looks_like_repo_root(&dir));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cargo_toml_alone_is_rejected() {
        let dir = uniq_dir("empty");
        write(&dir.join("Cargo.toml"), "[workspace]\n");
        assert!(!looks_like_repo_root(&dir));
        assert!(find_extracted_root(&dir).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn github_layout_nested_root_is_found() {
        let extract = uniq_dir("nested");
        let inner = extract.join("pi-midi-toolkit-deadbeef");
        native_tree(&inner);
        assert_eq!(find_extracted_root(&extract).unwrap(), inner);
        let _ = fs::remove_dir_all(extract);
    }

    #[test]
    fn overlay_skips_bin_and_data_root() {
        let src = uniq_dir("src");
        let dest = uniq_dir("dest");
        let data = uniq_dir("data");
        native_tree(&src);
        write(&src.join("bin/pidi-native"), "NEWBIN");
        write(&src.join("apps/pidi/wavetables/x.wav"), "wave");
        write(&src.join("README.md"), "hello");
        write(&dest.join("bin/pidi-native"), "OLDBIN");
        write(&data.join("settings.json"), "{}");

        overlay_tree(&src, &dest, &data).unwrap();

        assert_eq!(
            fs::read_to_string(dest.join("bin/pidi-native")).unwrap(),
            "OLDBIN",
            "live bin/ must not be overlay-copied"
        );
        assert_eq!(fs::read_to_string(dest.join("README.md")).unwrap(), "hello");
        assert!(dest.join("crates/pidi-native/src/lib.rs").is_file());
        assert!(dest.join("apps/pidi/wavetables/x.wav").is_file());
        assert_eq!(fs::read_to_string(data.join("settings.json")).unwrap(), "{}");

        let _ = fs::remove_dir_all(src);
        let _ = fs::remove_dir_all(dest);
        let _ = fs::remove_dir_all(data);
    }

    #[test]
    fn apply_extracted_replaces_changed_bins_and_reloads_kiosk() {
        let src = uniq_dir("apply-src");
        let dest = uniq_dir("apply-dest");
        let data = uniq_dir("apply-data");
        native_tree(&src);
        write(&src.join("dist/armv7/pidi-native"), "NEWUI");
        write(&src.join("dist/armv7/jambox-engine"), "NEWENG");
        write(&dest.join("bin/pidi-native"), "OLDUI");
        write(&dest.join("bin/jambox-engine"), "NEWENG");

        // dest needs to look like the live repo after overlay (src copied first)
        overlay_tree(&src, &dest, &data).unwrap();
        // restore live bins that overlay skipped
        write(&dest.join("bin/pidi-native"), "OLDUI");
        write(&dest.join("bin/jambox-engine"), "NEWENG");
        write(&dest.join("dist/armv7/pidi-native"), "NEWUI");
        write(&dest.join("dist/armv7/jambox-engine"), "NEWENG");

        let out = apply_extracted(&src, &dest, &data).unwrap();
        assert!(out.installed_bins.contains(&"pidi-native".into()));
        assert!(!out.installed_bins.contains(&"jambox-engine".into()));
        assert!(out.reload_kiosk);
        assert_eq!(fs::read_to_string(dest.join("bin/pidi-native")).unwrap(), "NEWUI");
        assert_eq!(
            fs::read_to_string(dest.join("bin/jambox-engine")).unwrap(),
            "NEWENG"
        );

        let _ = fs::remove_dir_all(src);
        let _ = fs::remove_dir_all(dest);
        let _ = fs::remove_dir_all(data);
    }

    #[test]
    fn should_skip_bin_prefix() {
        assert!(should_skip_overlay(Path::new("bin")));
        assert!(should_skip_overlay(Path::new("bin/pidi-native")));
        assert!(should_skip_overlay(Path::new("target/release/x")));
        assert!(!should_skip_overlay(Path::new("crates/pidi-native")));
        assert!(!should_skip_overlay(Path::new("dist/armv7/pidi-native")));
    }
}
