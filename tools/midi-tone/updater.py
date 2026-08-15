#!/usr/bin/env python3
"""Opt-in software update for the Pi MIDI box.

Checks the repo's default branch (master, then main). If the running copy
is behind, installs the **whole repo** the same way SSH deploy does:

* kiosk (``tools/midi-tone``)
* crates, deploy scripts, shipped presets
* restart ``midi-engine`` / ``jambox-engine`` when those units exist

Two layouts are supported:

* Full git clone of pi-midi-toolkit (``git fetch`` + fast-forward).
* Split install (``~/midi-tone`` kiosk + ``~/pi-midi-toolkit`` engines):
  download the branch archive, overlay the repo root, then overlay the
  running kiosk copy.

Never touches user data: ``settings.json``, ``songs/``, ``phrases/``,
``user-presets/``, ``user-wavetables/``, ``.venv/``, credentials,
``presets/active.json``, or existing ``bin/`` engine binaries.

Does **not** cargo-build on the Pi (Pi 2 is too slow). New Rust binaries
still come from a host cross-compile / SSH deploy.

The GitHub repo is private, so CHECK/UPDATE need a token (or a git remote
that already has credentials).
"""

from __future__ import annotations

import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Callable, Dict, Iterable, List, Optional, Sequence


HERE = pathlib.Path(__file__).resolve().parent
DEFAULT_REPO_URL = "https://github.com/HyruleanHero1988/pi-midi-toolkit.git"
DEFAULT_BRANCHES = ("master", "main")
MIDI_TONE_REL = pathlib.Path("tools") / "midi-tone"
VERSION_NAME = "version.json"
CREDENTIALS_NAME = ".update-credentials"
PI_CREDENTIALS_NAME = ".pi-credentials"
USER_AGENT = "midi-tone-updater/1.0"
GIT_TIMEOUT_SEC = 45
CHECK_TIMEOUT_SEC = 25
DOWNLOAD_TIMEOUT_SEC = 180
PIP_TIMEOUT_SEC = 300

# Names / relative prefixes an overlay must never replace or delete.
KEEP_KIOSK = frozenset(
    {
        "settings.json",
        "songs",
        "phrases",
        "user-presets",
        "user-wavetables",
        ".venv",
        ".pi-credentials",
        ".update-credentials",
        "version.json",
        "__pycache__",
        ".git",
        ".gitignore",
    }
)
KEEP_NAMES = KEEP_KIOSK  # alias used by tests / kiosk-only overlay
KEEP_REPO = frozenset(
    {
        ".git",
        ".venv",
        "target",
        "bin",  # SSH-deployed midi-engine / jambox-engine binaries
        "takes",
        "presets/active.json",
        "tools/midi-tone/settings.json",
        "tools/midi-tone/songs",
        "tools/midi-tone/phrases",
        "tools/midi-tone/user-presets",
        "tools/midi-tone/user-wavetables",
        "tools/midi-tone/.venv",
        "tools/midi-tone/.pi-credentials",
        "tools/midi-tone/.update-credentials",
        "tools/midi-tone/version.json",
        "tools/midi-tone/__pycache__",
    }
)

ProgressCb = Callable[[str], None]


class UpdateError(RuntimeError):
    """User-facing update failure (network, auth, git, overlay)."""


@dataclass
class VersionInfo:
    sha: str = ""
    branch: str = ""
    source: str = ""  # git | file | remote | unknown
    repo_url: str = ""

    @property
    def short(self) -> str:
        return (self.sha[:7] if self.sha else "unknown")


@dataclass
class Credentials:
    repo_url: str = DEFAULT_REPO_URL
    branch: str = ""
    token: str = ""


@dataclass
class UpdateCheck:
    local: VersionInfo
    remote: VersionInfo
    available: bool
    error: str = ""
    message: str = ""


def _noop_progress(_msg: str) -> None:
    return


def redact_url(url: str) -> str:
    """Strip userinfo (tokens) from a URL for logs / UI."""
    if not url:
        return ""
    return re.sub(r"(://)([^/@]+)@", r"\1***@", url)


def github_owner_repo(url: str) -> Optional[tuple[str, str]]:
    """Parse owner/repo from a GitHub HTTPS or SSH remote URL."""
    if not url:
        return None
    cleaned = re.sub(r"^git\+", "", url.strip())
    cleaned = re.sub(r"\.git$", "", cleaned)
    m = re.search(r"github\.com[:/](?P<owner>[^/]+)/(?P<repo>[^/#?\s]+)", cleaned)
    if not m:
        return None
    return m.group("owner"), m.group("repo")


def authenticated_https_url(url: str, token: str) -> str:
    """Embed a PAT in an HTTPS GitHub URL for git ls-remote / clone."""
    if not token:
        return url
    parsed = github_owner_repo(url)
    if parsed is None:
        return url
    owner, repo = parsed
    return f"https://x-access-token:{token}@github.com/{owner}/{repo}.git"


def _read_kv_file(path: pathlib.Path) -> Dict[str, str]:
    out: Dict[str, str] = {}
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return out
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        out[key.strip()] = value.strip().strip('"').strip("'")
    return out


def load_credentials(install: pathlib.Path = HERE) -> Credentials:
    """Env wins, then ``.update-credentials``, then ``.pi-credentials``."""
    creds = Credentials()
    merged: Dict[str, str] = {}
    for name in (PI_CREDENTIALS_NAME, CREDENTIALS_NAME):
        merged.update(_read_kv_file(install / name))
    env_url = os.environ.get("MIDI_TONE_REPO_URL", "").strip()
    env_branch = os.environ.get("MIDI_TONE_UPDATE_BRANCH", "").strip()
    env_token = (
        os.environ.get("MIDI_TONE_UPDATE_TOKEN", "").strip()
        or os.environ.get("GITHUB_TOKEN", "").strip()
    )
    creds.repo_url = (
        env_url
        or merged.get("REPO_URL", "").strip()
        or merged.get("MIDI_TONE_REPO_URL", "").strip()
        or DEFAULT_REPO_URL
    )
    creds.branch = (
        env_branch
        or merged.get("BRANCH", "").strip()
        or merged.get("MIDI_TONE_UPDATE_BRANCH", "").strip()
    )
    creds.token = (
        env_token
        or merged.get("GITHUB_TOKEN", "").strip()
        or merged.get("MIDI_TONE_UPDATE_TOKEN", "").strip()
        or merged.get("TOKEN", "").strip()
    )
    return creds


def save_token(token: str, install: pathlib.Path = HERE) -> pathlib.Path:
    """Write / update GITHUB_TOKEN in ``.update-credentials`` (gitignored)."""
    path = install / CREDENTIALS_NAME
    existing = _read_kv_file(path)
    existing["GITHUB_TOKEN"] = token.strip()
    if "REPO_URL" not in existing:
        existing["REPO_URL"] = DEFAULT_REPO_URL
    lines = [
        "# midi-tone kiosk update credentials (gitignored — do not commit)",
        f"REPO_URL={existing.get('REPO_URL', DEFAULT_REPO_URL)}",
        f"BRANCH={existing.get('BRANCH', '')}",
        f"GITHUB_TOKEN={token.strip()}",
        "",
    ]
    path.write_text("\n".join(lines), encoding="utf-8")
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass
    return path


def _run_git(
    args: Sequence[str],
    *,
    cwd: pathlib.Path,
    timeout: int = GIT_TIMEOUT_SEC,
    check: bool = True,
    extra_env: Optional[Dict[str, str]] = None,
) -> str:
    env = os.environ.copy()
    env["GIT_TERMINAL_PROMPT"] = "0"
    env["GIT_ASKPASS"] = "echo"
    env["GCM_INTERACTIVE"] = "never"
    if extra_env:
        env.update(extra_env)
    try:
        proc = subprocess.run(
            ["git", *args],
            cwd=str(cwd),
            timeout=timeout,
            check=False,
            capture_output=True,
            text=True,
            env=env,
        )
    except FileNotFoundError as exc:
        raise UpdateError("git is not installed on this box") from exc
    except subprocess.TimeoutExpired as exc:
        raise UpdateError(f"git timed out: {' '.join(args)}") from exc
    if check and proc.returncode != 0:
        err = (proc.stderr or proc.stdout or f"exit {proc.returncode}").strip()
        raise UpdateError(f"git {' '.join(args)} failed: {err}")
    return (proc.stdout or "").strip()


def git_available() -> bool:
    return shutil.which("git") is not None


def git_root(start: pathlib.Path) -> Optional[pathlib.Path]:
    if not git_available():
        return None
    try:
        top = _run_git(
            ["rev-parse", "--show-toplevel"],
            cwd=start,
            timeout=8,
        )
    except UpdateError:
        return None
    path = pathlib.Path(top)
    return path if path.is_dir() else None


def read_version_file(install: pathlib.Path) -> VersionInfo:
    path = install / VERSION_NAME
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return VersionInfo(source="unknown")
    if isinstance(data, str):
        return VersionInfo(sha=data.strip(), source="file")
    if not isinstance(data, dict):
        return VersionInfo(source="unknown")
    return VersionInfo(
        sha=str(data.get("sha") or "").strip(),
        branch=str(data.get("branch") or "").strip(),
        source=str(data.get("source") or "file"),
        repo_url=str(data.get("repo_url") or "").strip(),
    )


def write_version_file(install: pathlib.Path, info: VersionInfo) -> None:
    payload = {
        "sha": info.sha,
        "branch": info.branch,
        "source": info.source,
        "repo_url": redact_url(info.repo_url),
        "updated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    path = install / VERSION_NAME
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def local_version(install: pathlib.Path = HERE) -> VersionInfo:
    root = git_root(install)
    if root is not None:
        try:
            sha = _run_git(["rev-parse", "HEAD"], cwd=root, timeout=8)
            branch = _run_git(
                ["rev-parse", "--abbrev-ref", "HEAD"],
                cwd=root,
                timeout=8,
                check=False,
            )
            if branch == "HEAD":
                branch = ""
            url = _run_git(
                ["remote", "get-url", "origin"],
                cwd=root,
                timeout=8,
                check=False,
            )
            return VersionInfo(
                sha=sha,
                branch=branch,
                source="git",
                repo_url=url,
            )
        except UpdateError:
            pass
    info = read_version_file(install)
    if info.sha:
        return info
    return VersionInfo(source="unknown")


def _parse_ls_remote(output: str) -> str:
    for line in output.splitlines():
        line = line.strip()
        if not line:
            continue
        sha = line.split()[0]
        if re.fullmatch(r"[0-9a-f]{7,40}", sha):
            return sha
    return ""


def remote_head_via_git(
    repo_url: str,
    branch: str,
    token: str = "",
    *,
    timeout: int = CHECK_TIMEOUT_SEC,
) -> str:
    url = authenticated_https_url(repo_url, token)
    args = ["ls-remote", "--heads", url]
    if branch:
        args.append(branch)
    out = _run_git(args, cwd=HERE, timeout=timeout)
    sha = _parse_ls_remote(out)
    if not sha:
        raise UpdateError(f"no remote branch {branch or '(default)'} at {redact_url(repo_url)}")
    return sha


def remote_head_via_api(
    repo_url: str,
    branch: str,
    token: str = "",
    *,
    timeout: int = CHECK_TIMEOUT_SEC,
) -> str:
    parsed = github_owner_repo(repo_url)
    if parsed is None:
        raise UpdateError("not a GitHub URL — cannot use the API fallback")
    owner, repo = parsed
    api = f"https://api.github.com/repos/{owner}/{repo}/commits/{branch}"
    headers = {
        "User-Agent": USER_AGENT,
        "Accept": "application/vnd.github+json",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(api, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read()
    except urllib.error.HTTPError as exc:
        if exc.code in (401, 403, 404):
            raise UpdateError(
                "can't reach the private repo — add a GitHub token "
                "(SET → TOKEN, or .update-credentials)"
            ) from exc
        raise UpdateError(f"GitHub API error {exc.code}") from exc
    except urllib.error.URLError as exc:
        raise UpdateError(f"network error: {exc.reason}") from exc
    try:
        data = json.loads(raw.decode("utf-8"))
    except json.JSONDecodeError as exc:
        raise UpdateError("GitHub API returned non-JSON") from exc
    sha = str(data.get("sha") or "").strip()
    if not sha:
        raise UpdateError("GitHub API response had no commit SHA")
    return sha


def detect_branch(
    repo_url: str,
    token: str = "",
    preferred: str = "",
) -> str:
    if preferred:
        return preferred
    # Prefer the repo's HEAD if git can see it.
    if git_available():
        url = authenticated_https_url(repo_url, token)
        try:
            out = _run_git(
                ["ls-remote", "--symref", url, "HEAD"],
                cwd=HERE,
                timeout=CHECK_TIMEOUT_SEC,
                check=False,
            )
        except UpdateError:
            out = ""
        for line in out.splitlines():
            m = re.search(r"refs/heads/(\S+)", line)
            if m and line.lower().startswith("ref:"):
                return m.group(1)
    return DEFAULT_BRANCHES[0]


def remote_head(
    repo_url: str,
    branch: str,
    token: str = "",
    *,
    timeout: int = CHECK_TIMEOUT_SEC,
) -> VersionInfo:
    sha = ""
    last_err: Optional[Exception] = None
    if git_available():
        try:
            sha = remote_head_via_git(repo_url, branch, token, timeout=timeout)
        except UpdateError as exc:
            last_err = exc
    if not sha:
        try:
            sha = remote_head_via_api(repo_url, branch, token, timeout=timeout)
        except UpdateError as exc:
            last_err = exc
    if not sha:
        raise UpdateError(str(last_err) if last_err else "could not read remote HEAD")
    return VersionInfo(sha=sha, branch=branch, source="remote", repo_url=repo_url)


def check_for_update(install: pathlib.Path = HERE) -> UpdateCheck:
    creds = load_credentials(install)
    local = local_version(install)
    repo_url = creds.repo_url or local.repo_url or DEFAULT_REPO_URL
    try:
        branch = detect_branch(repo_url, creds.token, creds.branch or local.branch)
        remote = remote_head(repo_url, branch, creds.token)
    except UpdateError as exc:
        return UpdateCheck(
            local=local,
            remote=VersionInfo(branch=creds.branch, repo_url=repo_url, source="remote"),
            available=False,
            error=str(exc),
            message=str(exc),
        )
    if not local.sha:
        available = True
        message = f"No local version stamp — remote {branch} is {remote.short}"
    elif local.sha == remote.sha:
        available = False
        message = f"Already on {branch} {local.short}"
    else:
        available = True
        message = f"Update available: {local.short} → {remote.short} ({branch})"
    return UpdateCheck(
        local=local,
        remote=remote,
        available=available,
        message=message,
    )


def overlay_tree(
    src: pathlib.Path,
    dest: pathlib.Path,
    keep: Optional[Iterable[str]] = None,
) -> List[str]:
    """Copy files from ``src`` onto ``dest``, skipping keep prefixes.

    ``keep`` is a set of relative paths (file or directory). ``songs`` skips
    the whole songs tree; ``presets/active.json`` skips only that file.
    Returns the relative paths that were written.
    """
    if not src.is_dir():
        raise UpdateError(f"update tree missing: {src}")
    skip = {str(item).strip("/") for item in (keep if keep is not None else KEEP_KIOSK)}
    dest.mkdir(parents=True, exist_ok=True)
    written: List[str] = []
    for path in sorted(src.rglob("*")):
        rel = path.relative_to(src)
        parts = rel.parts
        if not parts:
            continue
        posix = rel.as_posix()
        if any(posix == item or posix.startswith(item + "/") for item in skip):
            continue
        if any(part == "__pycache__" or part.endswith(".pyc") for part in parts):
            continue
        target = dest / rel
        if path.is_dir():
            target.mkdir(parents=True, exist_ok=True)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, target)
        written.append(posix)
    return written


def looks_like_repo_root(path: pathlib.Path) -> bool:
    return (path / "Cargo.toml").is_file() and (path / MIDI_TONE_REL / "midi_tone.py").is_file()


def repo_root_for(install: pathlib.Path = HERE) -> pathlib.Path:
    """Directory that should receive the full pi-midi-toolkit tree."""
    env = os.environ.get("MIDI_TONE_REPO_ROOT", "").strip()
    if env:
        return pathlib.Path(env).expanduser().resolve()
    git = git_root(install)
    if git is not None:
        return git
    nested = install.parent.parent
    if looks_like_repo_root(nested):
        return nested
    home_repo = pathlib.Path.home() / "pi-midi-toolkit"
    if looks_like_repo_root(home_repo) or home_repo.is_dir():
        return home_repo
    sibling = install.parent / "pi-midi-toolkit"
    if looks_like_repo_root(sibling) or sibling.is_dir():
        return sibling
    return home_repo


def _find_repo_root(extracted: pathlib.Path) -> pathlib.Path:
    if looks_like_repo_root(extracted):
        return extracted
    for cargo in extracted.glob("*/Cargo.toml"):
        root = cargo.parent
        if looks_like_repo_root(root):
            return root
    raise UpdateError("downloaded archive was not a full pi-midi-toolkit tree")


def _sync_kiosk_from_repo(
    repo_root: pathlib.Path,
    install: pathlib.Path,
    progress: ProgressCb,
) -> None:
    src = repo_root / MIDI_TONE_REL
    if not (src / "midi_tone.py").is_file():
        return
    try:
        if src.resolve() == install.resolve():
            return
    except OSError:
        pass
    progress("Updating kiosk copy…")
    overlay_tree(src, install, keep=KEEP_KIOSK)


def _ensure_active_preset(repo_root: pathlib.Path) -> None:
    presets = repo_root / "presets"
    active = presets / "active.json"
    if active.is_file() or not presets.is_dir():
        return
    for name in ("mpk-mini-ch3.json", "example.json"):
        src = presets / name
        if src.is_file():
            shutil.copy2(src, active)
            return


def _restart_engines(progress: ProgressCb) -> None:
    """Restart mapper / jambox daemons if this box already has them.

    Matches SSH deploy's ``systemctl restart``. Fails soft: kiosk sudoers
    may only allow poweroff/reboot, and a unit may not be installed yet.
    """
    for unit in ("midi-engine", "jambox-engine"):
        try:
            enabled = subprocess.run(
                ["systemctl", "is-enabled", unit],
                capture_output=True,
                text=True,
                timeout=8,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            return
        if enabled.returncode != 0:
            continue
        progress(f"Restarting {unit}…")
        try:
            subprocess.run(
                ["sudo", "-n", "systemctl", "restart", unit],
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired):
            continue


def _chmod_scripts(install: pathlib.Path) -> None:
    scripts = [p for p in install.glob("*.sh") if p.is_file()]
    for path in scripts:
        try:
            mode = path.stat().st_mode
            path.chmod(mode | 0o111)
        except OSError:
            pass
    if not scripts:
        return
    try:
        subprocess.run(
            ["sed", "-i", r"s/\r$//"] + [p.name for p in scripts],
            cwd=str(install),
            timeout=10,
            check=False,
            capture_output=True,
        )
    except (OSError, subprocess.TimeoutExpired):
        pass


def _pip_install(install: pathlib.Path, progress: ProgressCb) -> None:
    pip = install / ".venv" / "bin" / "pip"
    req = install / "requirements.txt"
    if not pip.is_file() or not req.is_file():
        return
    progress("Installing Python packages…")
    try:
        proc = subprocess.run(
            [str(pip), "install", "-r", str(req)],
            cwd=str(install),
            timeout=PIP_TIMEOUT_SEC,
            check=False,
            capture_output=True,
            text=True,
        )
    except subprocess.TimeoutExpired as exc:
        raise UpdateError("pip install timed out") from exc
    if proc.returncode != 0:
        err = (proc.stderr or proc.stdout or f"exit {proc.returncode}").strip()
        raise UpdateError(f"pip install failed: {err[-400:]}")


def _git_fast_forward(
    root: pathlib.Path,
    branch: str,
    repo_url: str,
    token: str,
    progress: ProgressCb,
) -> str:
    progress(f"Fetching {branch}…")
    url = authenticated_https_url(repo_url, token) if token else ""
    fetch_args = ["fetch", "--tags", "origin", branch]
    extra_env = None
    # If origin is unreachable without a token, fetch from the authenticated URL.
    try:
        _run_git(fetch_args, cwd=root, timeout=DOWNLOAD_TIMEOUT_SEC)
    except UpdateError:
        if not url:
            raise
        progress("Fetching with token…")
        _run_git(
            ["fetch", "--tags", url, f"+refs/heads/{branch}:refs/remotes/origin/{branch}"],
            cwd=root,
            timeout=DOWNLOAD_TIMEOUT_SEC,
            extra_env=extra_env,
        )
    progress("Fast-forwarding…")
    _run_git(["merge", "--ff-only", f"origin/{branch}"], cwd=root, timeout=GIT_TIMEOUT_SEC)
    return _run_git(["rev-parse", "HEAD"], cwd=root, timeout=8)


def _find_midi_tone_dir(extracted: pathlib.Path) -> pathlib.Path:
    direct = extracted / MIDI_TONE_REL
    if (direct / "midi_tone.py").is_file():
        return direct
    matches = list(extracted.glob("*/tools/midi-tone/midi_tone.py"))
    if matches:
        return matches[0].parent
    matches = list(extracted.rglob("midi_tone.py"))
    for hit in matches:
        if hit.parent.name == "midi-tone":
            return hit.parent
    raise UpdateError("downloaded archive did not contain tools/midi-tone")


def _download_archive(
    repo_url: str,
    branch: str,
    token: str,
    dest_tar: pathlib.Path,
    progress: ProgressCb,
) -> None:
    parsed = github_owner_repo(repo_url)
    if parsed is None:
        raise UpdateError("not a GitHub URL — cannot download an archive")
    owner, repo = parsed
    url = f"https://codeload.github.com/{owner}/{repo}/tar.gz/refs/heads/{branch}"
    headers = {"User-Agent": USER_AGENT, "Accept": "application/octet-stream"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, headers=headers)
    progress("Downloading latest code…")
    try:
        with urllib.request.urlopen(req, timeout=DOWNLOAD_TIMEOUT_SEC) as resp:
            dest_tar.write_bytes(resp.read())
    except urllib.error.HTTPError as exc:
        if exc.code in (401, 403, 404):
            raise UpdateError(
                "download failed — private repo needs a GitHub token "
                "(SET → TOKEN)"
            ) from exc
        raise UpdateError(f"download failed (HTTP {exc.code})") from exc
    except urllib.error.URLError as exc:
        raise UpdateError(f"network error: {exc.reason}") from exc
    if dest_tar.stat().st_size < 64:
        raise UpdateError("download was empty")


def _safe_extract_tar(archive: pathlib.Path, dest: pathlib.Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    dest = dest.resolve()
    with tarfile.open(archive, "r:gz") as tf:
        for member in tf.getmembers():
            name = pathlib.Path(member.name)
            if name.is_absolute() or ".." in name.parts:
                raise UpdateError("refusing archive with unsafe paths")
            target = (dest / name).resolve()
            if dest not in target.parents and target != dest:
                raise UpdateError("refusing archive with unsafe paths")
        kwargs = {"filter": "data"} if sys.version_info >= (3, 12) else {}
        try:
            tf.extractall(dest, **kwargs)  # type: ignore[arg-type]
        except TypeError:
            tf.extractall(dest)


def apply_from_archive(
    install: pathlib.Path,
    repo_url: str,
    branch: str,
    token: str,
    progress: ProgressCb,
    repo_root: Optional[pathlib.Path] = None,
) -> str:
    with tempfile.TemporaryDirectory(prefix="midi-tone-update-") as tmp:
        tmp_path = pathlib.Path(tmp)
        tar_path = tmp_path / "src.tar.gz"
        _download_archive(repo_url, branch, token, tar_path, progress)
        progress("Unpacking…")
        extract_dir = tmp_path / "src"
        _safe_extract_tar(tar_path, extract_dir)
        src_root = _find_repo_root(extract_dir)
        dest_root = repo_root or repo_root_for(install)
        progress("Installing full repo…")
        overlay_tree(src_root, dest_root, keep=KEEP_REPO)
        _ensure_active_preset(dest_root)
        _sync_kiosk_from_repo(dest_root, install, progress)
        return ""


def apply_update(
    install: pathlib.Path = HERE,
    *,
    progress: Optional[ProgressCb] = None,
    expected_sha: str = "",
) -> VersionInfo:
    """Install the remote branch into the repo + running kiosk. Returns the new version."""
    log = progress or _noop_progress
    creds = load_credentials(install)
    repo_url = creds.repo_url or DEFAULT_REPO_URL
    branch = detect_branch(repo_url, creds.token, creds.branch)
    remote = remote_head(repo_url, branch, creds.token)
    if expected_sha and remote.sha != expected_sha:
        raise UpdateError("remote moved while checking — tap UPDATE again")
    local = local_version(install)
    if local.sha and local.sha == remote.sha:
        write_version_file(install, remote)
        log("Already on latest.")
        return remote

    dest_root = repo_root_for(install)
    git = git_root(dest_root) or git_root(install)
    used_git = False
    if git is not None:
        try:
            sha = _git_fast_forward(git, branch, repo_url, creds.token, log)
            used_git = True
            remote.sha = sha
            remote.source = "git"
            dest_root = git
            _sync_kiosk_from_repo(git, install, log)
        except UpdateError as exc:
            log(f"git pull skipped ({exc}); downloading archive…")

    if not used_git:
        apply_from_archive(
            install, repo_url, branch, creds.token, log, repo_root=dest_root
        )
        remote.source = "archive"

    _chmod_scripts(install)
    _chmod_scripts(dest_root / MIDI_TONE_REL)
    _pip_install(install, log)
    _ensure_active_preset(dest_root)
    _restart_engines(log)
    write_version_file(install, remote)
    try:
        write_version_file(dest_root / MIDI_TONE_REL, remote)
    except OSError:
        pass
    log(f"Installed {branch} {remote.short} (full repo)")
    return remote


def restart_current_process(argv: Optional[Iterable[str]] = None) -> None:
    """Replace this process with a fresh midi_tone.py (same PID / kiosk loop)."""
    python = sys.executable or "python3"
    script = str(HERE / "midi_tone.py")
    args = [python, "-u", script]
    if argv is None:
        args.extend(sys.argv[1:])
    else:
        args.extend(list(argv))
    os.chdir(str(HERE))
    os.execv(python, args)


def format_status_lines(check: Optional[UpdateCheck] = None, install: pathlib.Path = HERE) -> str:
    local = check.local if check else local_version(install)
    creds = load_credentials(install)
    has_token = bool(creds.token)
    lines = [
        f"Running: {local.short}"
        + (f"  ({local.branch})" if local.branch else "")
        + (f"  via {local.source}" if local.source and local.source != "unknown" else ""),
    ]
    if check is None:
        lines.append("Remote: tap CHECK to look at GitHub.")
    elif check.error:
        lines.append(f"Remote: {check.error}")
    else:
        remote = check.remote
        lines.append(
            f"Remote {remote.branch}: {remote.short}"
            + (" — UPDATE available" if check.available else " — up to date")
        )
    if not has_token and (check is None or check.error):
        lines.append("Private repo: add a GitHub token (TOKEN) if CHECK fails.")
    lines.append("Full deploy: kiosk + crates + presets. Keeps songs / phrases / settings.")
    return "\n".join(lines)


def main(argv: Optional[Sequence[str]] = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(description="midi-tone kiosk updater")
    parser.add_argument("--status", action="store_true", help="print local version")
    parser.add_argument("--check", action="store_true", help="compare to remote branch")
    parser.add_argument("--apply", action="store_true", help="install remote if behind")
    args = parser.parse_args(list(argv) if argv is not None else None)

    if args.apply:
        def _print(msg: str) -> None:
            print(msg, flush=True)

        info = apply_update(HERE, progress=_print)
        print(f"now {info.short} ({info.branch})")
        return 0
    if args.check:
        result = check_for_update(HERE)
        print(result.message or result.error)
        print(f"local  {result.local.sha or 'unknown'}")
        print(f"remote {result.remote.sha or 'unknown'}")
        return 0 if not result.error else 1
    info = local_version(HERE)
    print(format_status_lines())
    print(f"sha {info.sha or 'unknown'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
