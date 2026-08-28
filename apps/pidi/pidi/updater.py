#!/usr/bin/env python3
"""Opt-in software update for the Pi MIDI box (native kiosk).

Checks the repo's default branch (master, then main). If the running copy
is behind, installs the **whole repo** the same way SSH deploy does:

* shared ``apps/pidi`` assets (wavetables, updater, HW scripts)
* crates, deploy scripts, shipped presets
* restart ``midi-engine`` / ``jambox-engine`` / ``pidi-native`` when those units exist

Layouts:

* Full git clone of pi-midi-toolkit (``git fetch`` + fast-forward).
* Split install (legacy ``~/midi-tone`` asset copy + ``~/pi-midi-toolkit``):
  download the branch archive, overlay the repo root, then sync ``apps/pidi``.

Never touches user data: ``settings.json``, ``songs/``, ``phrases/``,
``user-presets/``, ``user-wavetables/``, credentials, ``presets/active.json``.
Live ``bin/`` is not overlay-copied; after the tree is in place, committed
``dist/armv7/{midi-engine,jambox-engine,pidi-native}`` are installed onto
``bin/`` via stop + atomic rename.

Component digests (``ui``, ``engines``) are stamped in ``version.json``.
Does **not** cargo-build on the Pi. CI rebuilds ``dist/armv7`` on master.

``pidi-native`` shells this module with ``--check`` (and optionally ``--apply``).
"""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from typing import Callable, Dict, Iterable, List, Optional, Sequence


HERE = pathlib.Path(__file__).resolve().parents[1]  # deploy root (apps/pidi or ~/midi-tone)
DEFAULT_REPO_URL = "https://github.com/HyruleanHero1988/pi-midi-toolkit.git"
DEFAULT_BRANCHES = ("master", "main")
MIDI_TONE_REL = pathlib.Path("apps") / "pidi"
VERSION_NAME = "version.json"
CREDENTIALS_NAME = ".update-credentials"
PI_CREDENTIALS_NAME = ".pi-credentials"
USER_AGENT = "midi-tone-updater/1.0"
GIT_TIMEOUT_SEC = 45
CHECK_TIMEOUT_SEC = 25
DOWNLOAD_TIMEOUT_SEC = 180
PIP_TIMEOUT_SEC = 300

# Overlay is copy-only and skips these paths so SET→UPDATE cannot wipe a live box.
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
KEEP_NAMES = KEEP_KIOSK
PI_ENGINE_BINS = ("midi-engine", "jambox-engine", "pidi-native")
STAGED_BIN_DIR = pathlib.Path("dist") / "armv7"
# Shared apps/pidi assets that count as the ``ui`` / appliance-support digest.
UI_DIGEST_PATHS: Sequence[str] = (
    "pidi",
    "scripts",
    "wavetables",
    "README.md",
    ".wifi-credentials.example",
)
KEEP_REPO = frozenset(
    {
        ".git",
        ".venv",
        "target",
        "bin",  # live engines; installed from dist/armv7 after overlay
        "takes",
        "presets/active.json",
        *(f"apps/pidi/{name}" for name in KEEP_KIOSK if name not in {".git", ".gitignore"}),
    }
)

ProgressCb = Callable[[str], None]


class UpdateError(RuntimeError):
    """User-facing update failure (network, auth, git, overlay)."""


# Rough OTA timeline markers (message substring → approximate %).
# More specific needles first. Percentages only move forward.
_UPDATE_PCT_MARKERS: Sequence[tuple[str, int]] = (
    ("Already on latest", 100),
    ("(full repo)", 100),
    ("Engines unchanged", 95),
    ("Requirements unchanged", 75),
    ("UI unchanged", 65),
    ("unchanged — skip", 90),
    ("Restarting", 95),
    ("→ bin/", 90),
    ("Stopping", 88),
    ("Installing Python packages", 72),
    ("Updating kiosk copy", 62),
    ("Installing full repo", 52),
    ("Unpacking", 42),
    ("Downloading latest code", 12),
    ("git pull skipped", 10),
    ("Fast-forwarding", 28),
    ("Fetching from GitHub", 12),
    ("Fetching ", 8),
)


def format_elapsed(seconds: float) -> str:
    """Format a duration as ``m:ss`` or ``h:mm:ss``."""
    total = max(0, int(seconds))
    hours, rem = divmod(total, 3600)
    minutes, secs = divmod(rem, 60)
    if hours:
        return f"{hours}:{minutes:02d}:{secs:02d}"
    return f"{minutes}:{secs:02d}"


def estimate_update_pct(msg: str) -> int:
    """Map a progress message to a rough completion percentage (0–100)."""
    text = (msg or "").strip()
    if not text:
        return 0
    # Download byte progress: "Downloading latest code… 3.1/12.0 MB"
    if text.startswith("Downloading latest code"):
        m = re.search(
            r"([\d.]+)\s*/\s*([\d.]+)\s*MB",
            text,
            flags=re.IGNORECASE,
        )
        if m:
            try:
                done = float(m.group(1))
                total = float(m.group(2))
            except ValueError:
                return 12
            if total > 0:
                frac = min(1.0, max(0.0, done / total))
                # Map download into ~12%–40% of the overall bar.
                return 12 + int(28 * frac)
        return 12
    for needle, pct in _UPDATE_PCT_MARKERS:
        if needle in text:
            return pct
    return 0


class ProgressTracker:
    """Wrap a progress sink with ``[pct% · elapsed]`` prefixes.

    Call ``tick()`` from the UI once a second so elapsed time advances
    even while a long download/pip step is silent.
    """

    def __init__(self, sink: ProgressCb) -> None:
        self._sink = sink
        self._t0 = time.monotonic()
        self._pct = 0
        self._msg = "Starting…"
        self._lock = threading.Lock()

    @property
    def started_at(self) -> float:
        return self._t0

    @property
    def elapsed_sec(self) -> float:
        return max(0.0, time.monotonic() - self._t0)

    def __call__(self, msg: str) -> None:
        text = (msg or "").strip() or "Working…"
        with self._lock:
            guessed = estimate_update_pct(text)
            if guessed:
                self._pct = max(self._pct, guessed)
            self._msg = text
            line = self._format_locked()
        self._sink(line)

    def tick(self) -> None:
        with self._lock:
            line = self._format_locked()
        self._sink(line)

    def _format_locked(self) -> str:
        elapsed = format_elapsed(time.monotonic() - self._t0)
        return f"[{self._pct}% · {elapsed}] {self._msg}"


@dataclass
class ComponentDigests:
    """Content hashes for independently skippable update pieces."""

    ui: str = ""
    engines: str = ""
    requirements: str = ""

    def as_dict(self) -> Dict[str, str]:
        return {
            "ui": self.ui,
            "engines": self.engines,
            "requirements": self.requirements,
        }

    @classmethod
    def from_mapping(cls, data: object) -> "ComponentDigests":
        if not isinstance(data, dict):
            return cls()
        return cls(
            ui=str(data.get("ui") or "").strip(),
            engines=str(data.get("engines") or "").strip(),
            requirements=str(data.get("requirements") or "").strip(),
        )

    @property
    def any(self) -> bool:
        return bool(self.ui or self.engines or self.requirements)


@dataclass
class VersionInfo:
    sha: str = ""
    branch: str = ""
    source: str = ""  # git | file | remote | unknown
    repo_url: str = ""
    components: ComponentDigests = field(default_factory=ComponentDigests)

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
        if not start.is_dir():
            return None
    except OSError:
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
        components=ComponentDigests.from_mapping(data.get("components")),
    )


def write_version_file(install: pathlib.Path, info: VersionInfo) -> None:
    payload = {
        "sha": info.sha,
        "branch": info.branch,
        "source": info.source,
        "repo_url": redact_url(info.repo_url),
        "updated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "components": info.components.as_dict(),
    }
    path = install / VERSION_NAME
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)


def _sha256_file(path: pathlib.Path) -> bytes:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    return digest.digest()


def _files_identical(left: pathlib.Path, right: pathlib.Path) -> bool:
    try:
        if not left.is_file() or not right.is_file():
            return False
        if left.stat().st_size != right.stat().st_size:
            return False
        return _sha256_file(left) == _sha256_file(right)
    except OSError:
        return False


def _iter_digest_files(root: pathlib.Path, rel: str) -> Iterable[pathlib.Path]:
    path = root / rel
    if path.is_file():
        yield path
        return
    if not path.is_dir():
        return
    for child in sorted(path.rglob("*")):
        if not child.is_file():
            continue
        parts = child.relative_to(root).parts
        if any(part == "__pycache__" or part.endswith(".pyc") for part in parts):
            continue
        posix = child.relative_to(root).as_posix()
        if any(posix == item or posix.startswith(item + "/") for item in KEEP_KIOSK):
            continue
        yield child


def _digest_named_paths(root: pathlib.Path, names: Sequence[str]) -> str:
    digest = hashlib.sha256()
    found = False
    for name in names:
        for path in _iter_digest_files(root, name):
            found = True
            rel = path.relative_to(root).as_posix()
            digest.update(rel.encode("utf-8"))
            digest.update(b"\0")
            digest.update(_sha256_file(path))
            digest.update(b"\0")
    return digest.hexdigest() if found else ""


def _looks_like_apps_pidi(path: pathlib.Path) -> bool:
    return (path / "pidi" / "updater.py").is_file() or (path / "wavetables").is_dir()


def kiosk_root_for(
    install: pathlib.Path,
    repo_root: Optional[pathlib.Path] = None,
) -> pathlib.Path:
    """Resolve the shared ``apps/pidi`` tree (or a legacy split copy)."""
    if _looks_like_apps_pidi(install):
        return install
    if repo_root is not None:
        candidate = repo_root / MIDI_TONE_REL
        if _looks_like_apps_pidi(candidate):
            return candidate
    return install


def digest_engines(repo_root: pathlib.Path) -> str:
    """Hash staged ``dist/armv7`` engines (fallback: live ``bin/``)."""
    digest = hashlib.sha256()
    found = False
    for name in PI_ENGINE_BINS:
        staged = repo_root / STAGED_BIN_DIR / name
        live = repo_root / "bin" / name
        path = staged if staged.is_file() else live
        digest.update(name.encode("utf-8"))
        digest.update(b"\0")
        if path.is_file():
            found = True
            digest.update(_sha256_file(path))
        else:
            digest.update(b"missing")
        digest.update(b"\0")
    return digest.hexdigest() if found else ""


def digest_requirements(kiosk: pathlib.Path) -> str:
    path = kiosk / "requirements.txt"
    if not path.is_file():
        return ""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def digest_ui(kiosk: pathlib.Path) -> str:
    return _digest_named_paths(kiosk, UI_DIGEST_PATHS)


def compute_component_digests(
    repo_root: pathlib.Path,
    install: Optional[pathlib.Path] = None,
    *,
    from_live_kiosk: bool = False,
) -> ComponentDigests:
    """Content digests for ui / engines / requirements under this layout.

    By default prefers ``repo_root/apps/pidi`` (what the commit ships).
    Pass ``from_live_kiosk=True`` when capturing the running box before an
    update (fallback when ``version.json`` has no stamp yet).
    """
    repo_kiosk = repo_root / MIDI_TONE_REL
    if from_live_kiosk:
        kiosk = kiosk_root_for(install or repo_root, repo_root)
    elif _looks_like_apps_pidi(repo_kiosk):
        kiosk = repo_kiosk
    else:
        kiosk = kiosk_root_for(install or repo_root, repo_root)
    return ComponentDigests(
        ui=digest_ui(kiosk),
        engines=digest_engines(repo_root),
        requirements=digest_requirements(kiosk),
    )


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
                f"can't reach GitHub (HTTP {exc.code}) — check network / branch name"
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

    Copy-only, like ``deploy_pi.py`` sftp.put: never deletes destination
    files. ``keep`` is a set of relative paths (file or directory).
    ``phrases`` skips the whole phrase-pad tree; ``presets/active.json``
    skips only that file.
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
    return (path / "Cargo.toml").is_file() and _looks_like_apps_pidi(path / MIDI_TONE_REL)


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
    if not _looks_like_apps_pidi(src):
        return
    try:
        if src.resolve() == install.resolve():
            return
    except OSError:
        pass
    progress("Updating apps/pidi asset copy…")
    overlay_tree(src, install, keep=KEEP_KIOSK)


def _engine_unit_enabled(unit: str) -> bool:
    try:
        enabled = subprocess.run(
            ["systemctl", "is-enabled", unit],
            capture_output=True,
            text=True,
            timeout=8,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return enabled.returncode == 0


def _systemctl(action: str, unit: str) -> None:
    try:
        subprocess.run(
            ["sudo", "-n", "systemctl", action, unit],
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        pass


def _stop_engines(progress: ProgressCb = _noop_progress) -> None:
    """Stop live engines so ``bin/`` can be replaced without ETXTBSY.

    Handles both systemd units and kiosk-spawned children (``MIDI_TONE_SPAWN``).
    """
    if os.name == "nt":
        return
    stopped = False
    # Kiosk first so it is not talking to an engine we are about to replace.
    for unit in reversed(PI_ENGINE_BINS):
        if not _engine_unit_enabled(unit):
            continue
        progress(f"Stopping {unit}…")
        _systemctl("stop", unit)
        stopped = True
    for name in PI_ENGINE_BINS:
        try:
            result = subprocess.run(
                ["pkill", "-x", name],
                capture_output=True,
                text=True,
                timeout=8,
                check=False,
            )
            if result.returncode == 0:
                stopped = True
        except (OSError, subprocess.TimeoutExpired):
            continue
    if stopped:
        # Brief pause so the kernel releases the mapped text segment.
        time.sleep(0.35)


def _install_one_binary(src: pathlib.Path, dest: pathlib.Path) -> None:
    """Install ``src`` onto ``dest`` without truncating a busy executable.

    ``shutil.copy2(src, dest)`` opens the existing path for write and hits
    ``Errno 26 (ETXTBSY)`` while systemd/kiosk still has the binary mapped.
    Writing beside it and ``os.replace`` swaps the directory entry; the old
    inode stays alive for any still-running process until it exits.
    """
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_name(dest.name + ".ota-new")
    try:
        if tmp.exists():
            tmp.unlink()
        shutil.copy2(src, tmp)
        if os.name != "nt":
            tmp.chmod(tmp.stat().st_mode | 0o111)
        os.replace(tmp, dest)
    finally:
        if tmp.exists():
            try:
                tmp.unlink()
            except OSError:
                pass


def install_pi_binaries(
    repo_root: pathlib.Path,
    progress: ProgressCb = _noop_progress,
) -> List[str]:
    """Copy committed ``dist/armv7`` engines onto live ``bin/``.

    Overlay never writes ``bin/`` (KEEP_REPO) so a half-applied update
    cannot clobber the running systemd paths with a host-arch file.
    Missing staged files leave the existing binary in place — same as
    SSH deploy when you only rebuilt one crate.

    Stops running engines first, then installs via atomic rename so a
    still-busy binary cannot raise ``[Errno 26] Text file busy``.
    """
    src_dir = repo_root / STAGED_BIN_DIR
    dest_dir = repo_root / "bin"
    if not src_dir.is_dir():
        progress("No dist/armv7 engines in this build — leaving existing bin/.")
        return []
    pending: List[str] = []
    for name in PI_ENGINE_BINS:
        src = src_dir / name
        if not src.is_file():
            continue
        dest = dest_dir / name
        if _files_identical(src, dest):
            progress(f"{name} unchanged — skip")
            continue
        pending.append(name)
    if not pending:
        progress("Engines unchanged — leaving bin/")
        return []
    _stop_engines(progress)
    dest_dir.mkdir(parents=True, exist_ok=True)
    installed: List[str] = []
    for name in pending:
        src = src_dir / name
        dest = dest_dir / name
        _install_one_binary(src, dest)
        installed.append(name)
        progress(f"Installed {name} → bin/")
    return installed


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
    """Restart mapper / jambox / native kiosk units if this box has them.

    Engines first, then ``pidi-native``. Fails soft: kiosk sudoers
    may only allow poweroff/reboot, and a unit may not be installed yet.
    """
    for unit in PI_ENGINE_BINS:
        if not _engine_unit_enabled(unit):
            continue
        progress(f"Restarting {unit}…")
        _systemctl("restart", unit)


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
        progress("Fetching from GitHub…")
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
    """Locate ``apps/pidi`` inside a GitHub source archive (name is historical)."""
    direct = extracted / MIDI_TONE_REL
    if _looks_like_apps_pidi(direct):
        return direct
    matches = list(extracted.glob("*/apps/pidi/pidi/updater.py"))
    if matches:
        return matches[0].parent.parent
    matches = list(extracted.glob("*/apps/pidi/wavetables"))
    if matches:
        return matches[0].parent
    raise UpdateError("downloaded archive did not contain apps/pidi")


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
            total = 0
            try:
                total = int(resp.headers.get("Content-Length") or 0)
            except (TypeError, ValueError):
                total = 0
            chunk_size = 256 * 1024
            read = 0
            last_report = -1.0
            with dest_tar.open("wb") as out:
                while True:
                    chunk = resp.read(chunk_size)
                    if not chunk:
                        break
                    out.write(chunk)
                    read += len(chunk)
                    now = time.monotonic()
                    if total > 0 and (now - last_report) >= 0.4:
                        last_report = now
                        mb = read / (1024 * 1024)
                        total_mb = total / (1024 * 1024)
                        progress(
                            f"Downloading latest code… {mb:.1f}/{total_mb:.1f} MB"
                        )
                    elif total <= 0 and (now - last_report) >= 1.0:
                        last_report = now
                        mb = read / (1024 * 1024)
                        progress(f"Downloading latest code… {mb:.1f} MB")
            if total > 0:
                progress(
                    f"Downloading latest code… "
                    f"{read / (1024 * 1024):.1f}/{total / (1024 * 1024):.1f} MB"
                )
    except urllib.error.HTTPError as exc:
        if exc.code in (401, 403, 404):
            raise UpdateError(
                f"download failed (HTTP {exc.code}) — check network / branch name"
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
    *,
    sync_kiosk: bool = True,
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
        if sync_kiosk:
            _sync_kiosk_from_repo(dest_root, install, progress)
        return ""


def _capture_prior_components(
    install: pathlib.Path,
    repo_root: pathlib.Path,
) -> ComponentDigests:
    """Digests from the last stamp, or computed from disk before mutation."""
    stamped = read_version_file(install)
    if stamped.components.any:
        return stamped.components
    try:
        return compute_component_digests(
            repo_root, install=install, from_live_kiosk=True
        )
    except OSError:
        return ComponentDigests()


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
    dest_root = repo_root_for(install)
    prior = _capture_prior_components(install, dest_root)
    local = local_version(install)
    if local.sha and local.sha == remote.sha:
        remote.components = compute_component_digests(dest_root, install=install)
        write_version_file(install, remote)
        log("Already on latest.")
        return remote

    git = git_root(dest_root) or git_root(install)
    used_git = False
    if git is not None:
        try:
            sha = _git_fast_forward(git, branch, repo_url, creds.token, log)
            used_git = True
            remote.sha = sha
            remote.source = "git"
            dest_root = git
        except UpdateError as exc:
            log(f"git pull skipped ({exc}); downloading archive…")

    if not used_git:
        apply_from_archive(
            install,
            repo_url,
            branch,
            creds.token,
            log,
            repo_root=dest_root,
            sync_kiosk=False,
        )
        remote.source = "archive"

    new = compute_component_digests(dest_root, install=install)
    remote.components = new
    changed: List[str] = []
    skipped: List[str] = []

    if new.ui != prior.ui:
        _sync_kiosk_from_repo(dest_root, install, log)
        _chmod_scripts(install)
        _chmod_scripts(dest_root / MIDI_TONE_REL)
        changed.append("ui")
    else:
        log("UI unchanged — skipping kiosk copy")
        skipped.append("ui")

    _ensure_active_preset(dest_root)

    if new.requirements != prior.requirements:
        _pip_install(install, log)
        changed.append("requirements")
    else:
        log("Requirements unchanged — skipping pip")
        skipped.append("requirements")

    if new.engines != prior.engines:
        installed = install_pi_binaries(dest_root, log)
        if installed:
            _restart_engines(log)
            changed.append("engines")
        else:
            # Digests differed (e.g. missing stamp) but live files already match.
            log("Engines already match staged binaries — skip restart")
            skipped.append("engines")
    else:
        log("Engines unchanged — leaving bin/")
        skipped.append("engines")

    write_version_file(install, remote)
    try:
        write_version_file(dest_root / MIDI_TONE_REL, remote)
    except OSError:
        pass
    parts = []
    if changed:
        parts.append("updated " + "+".join(changed))
    if skipped:
        parts.append("skipped " + "+".join(skipped))
    summary = "; ".join(parts) if parts else "no component changes"
    log(f"Installed {branch} {remote.short} ({summary})")
    return remote


def format_running_version_line(install: pathlib.Path = HERE) -> str:
    """Short git/deploy stamp for status UIs."""
    stamped = read_version_file(install)
    local = stamped if stamped.sha else local_version(install)
    line = "PiDI"
    if local.short and local.short not in ("unknown", ""):
        line += f"  ·  {local.short}"
    return line


def format_status_lines(check: Optional[UpdateCheck] = None, install: pathlib.Path = HERE) -> str:
    if check is not None:
        local = check.local
    else:
        stamped = read_version_file(install)
        local = stamped if stamped.sha else local_version(install)
    lines = [
        "PiDI",
        f"Running: {local.short}"
        + (f"  ({local.branch})" if local.branch else "")
        + (f"  via {local.source}" if local.source and local.source != "unknown" else ""),
    ]
    if check is None:
        lines.append("Remote: —")
    elif check.error:
        lines.append(f"Remote: {check.error}")
    else:
        remote = check.remote
        lines.append(
            f"Remote {remote.branch}: {remote.short}"
            + (" — UPDATE available" if check.available else " — up to date")
        )
    return "\n".join(lines)


def main(argv: Optional[Sequence[str]] = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(description="PiDI / pidi-native updater")
    parser.add_argument("--status", action="store_true", help="print local version")
    parser.add_argument("--check", action="store_true", help="compare to remote branch")
    parser.add_argument("--apply", action="store_true", help="install remote if behind")
    args = parser.parse_args(list(argv) if argv is not None else None)

    if args.apply:
        def _print(msg: str) -> None:
            print(msg, flush=True)

        info = apply_update(HERE, progress=ProgressTracker(_print))
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
