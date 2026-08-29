#!/usr/bin/env python3
"""Deploy the local repo tree + native armv7 bins to the lab Pi.

Uses ``apps/pidi/.pi-credentials`` (gitignored). Overlays source onto the Pi
while preserving user data, then installs ``dist/armv7`` engines and restarts
``jambox-engine`` + ``pidi-native``.

Run from anywhere::

    python apps/pidi/scripts/deploy/deploy_native.py
    python apps/pidi/scripts/deploy/deploy_native.py --no-restart
    python apps/pidi/scripts/deploy/deploy_native.py --branch feat/my-work

Optional ``.pi-credentials`` keys (defaults shown for user ``pi``)::

    PI_REPO=/home/pi/pi-midi-toolkit
    PI_DIR=~/midi-tone
    PIDI_DATA_ROOT=/home/pi/.local/share/pidi
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import pathlib
import subprocess
import sys
import tarfile
import time

try:
    import paramiko
except ImportError:
    sys.exit("pip install paramiko")

HERE = pathlib.Path(__file__).resolve().parents[2]  # apps/pidi
ROOT = HERE.parents[1]  # repo root
CREDS_PATH = HERE / ".pi-credentials"
STAGE = ROOT / "dist" / "armv7"

KEEP_REPO = {
    ".git",
    ".venv",
    "target",
    "bin",
    "takes",
    "presets/active.json",
    "apps/pidi/settings.json",
    "apps/pidi/songs",
    "apps/pidi/phrases",
    "apps/pidi/user-presets",
    "apps/pidi/user-wavetables",
    "apps/pidi/.venv",
    "apps/pidi/.pi-credentials",
    "apps/pidi/.update-credentials",
    "apps/pidi/version.json",
}

KEEP_KIOSK = {
    "settings.json",
    "songs",
    "phrases",
    "user-presets",
    "user-wavetables",
    ".venv",
    ".pi-credentials",
    ".update-credentials",
    "version.json",
    "bin",
}

TAR_SKIP_PARTS = {
    ".git",
    "target",
    ".cursor",
    "__pycache__",
    "_pi-user-backup",
    "node_modules",
}

ENGINE_BINS = ("jambox-engine", "pidi-native", "midi-engine")


def load_creds() -> dict[str, str]:
    if not CREDS_PATH.is_file():
        sys.exit(f"Missing {CREDS_PATH}")
    creds: dict[str, str] = {}
    for line in CREDS_PATH.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        creds[k.strip()] = v.strip()
    for key in ("PI_HOST", "PI_USER", "PI_PASSWORD"):
        if key not in creds:
            sys.exit(f"Missing {key} in {CREDS_PATH}")
    return creds


def remote_paths(creds: dict[str, str]) -> tuple[str, str, str]:
    user = creds["PI_USER"]
    repo = creds.get("PI_REPO", f"/home/{user}/pi-midi-toolkit")
    kiosk = creds.get("PI_DIR", "~/midi-tone").replace("~", f"/home/{user}")
    data = creds.get("PIDI_DATA_ROOT", f"/home/{user}/.local/share/pidi")
    return repo, kiosk, data


def git_sha() -> str:
    try:
        out = subprocess.check_output(
            ["git", "-C", str(ROOT), "rev-parse", "HEAD"],
            text=True,
            stderr=subprocess.DEVNULL,
        )
        return out.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return "unknown"


def git_branch() -> str:
    try:
        out = subprocess.check_output(
            ["git", "-C", str(ROOT), "branch", "--show-current"],
            text=True,
            stderr=subprocess.DEVNULL,
        )
        branch = out.strip()
        if branch:
            return branch
    except (subprocess.CalledProcessError, FileNotFoundError):
        pass
    try:
        out = subprocess.check_output(
            ["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
            text=True,
            stderr=subprocess.DEVNULL,
        )
        return f"detached@{out.strip()}"
    except (subprocess.CalledProcessError, FileNotFoundError):
        return "unknown"


def should_skip(rel: pathlib.Path) -> bool:
    parts = rel.parts
    if not parts:
        return True
    if any(p in TAR_SKIP_PARTS for p in parts):
        return True
    return rel.suffix == ".pyc"


def build_tar() -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        for path in sorted(ROOT.rglob("*")):
            rel = path.relative_to(ROOT)
            if should_skip(rel):
                continue
            tar.add(path, arcname=str(rel).replace("\\", "/"))
    return buf.getvalue()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def connect(creds: dict[str, str]) -> paramiko.SSHClient:
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(
        creds["PI_HOST"],
        username=creds["PI_USER"],
        password=creds["PI_PASSWORD"],
        timeout=60,
        allow_agent=False,
        look_for_keys=False,
    )
    return client


def run(client: paramiko.SSHClient, cmd: str, *, timeout: int = 300) -> tuple[int, str]:
    print(f"$ {cmd}", flush=True)
    _stdin, stdout, stderr = client.exec_command(cmd, timeout=timeout, get_pty=True)
    out = stdout.read().decode(errors="replace")
    err = stderr.read().decode(errors="replace")
    code = stdout.channel.recv_exit_status()
    text = (out + err).strip()
    if text:
        print(text, flush=True)
    print(f"exit {code}", flush=True)
    return code, text


def sudo(client: paramiko.SSHClient, password: str, cmd: str, timeout: int = 300) -> tuple[int, str]:
    return run(client, f"echo '{password}' | sudo -S -p '' {cmd}", timeout=timeout)


def sftp_put_bytes(client: paramiko.SSHClient, data: bytes, remote: str) -> None:
    print(f"put {len(data)} bytes -> {remote}", flush=True)
    sftp = client.open_sftp()
    try:
        with sftp.file(remote, "wb") as f:
            f.write(data)
    finally:
        sftp.close()


def sftp_put_file(
    client: paramiko.SSHClient,
    local: pathlib.Path,
    remote: str,
    *,
    mode: int = 0o755,
) -> None:
    print(f"put {local.name} -> {remote} ({local.stat().st_size} bytes)", flush=True)
    sftp = client.open_sftp()
    try:
        tmp = remote + ".new"
        sftp.put(str(local), tmp)
        sftp.chmod(tmp, mode)
    finally:
        sftp.close()
    run(client, f"mv -f '{tmp}' '{remote}'")


def overlay_script() -> str:
    keep_repo = " ".join(f"'{x}'" for x in sorted(KEEP_REPO))
    keep_kiosk = " ".join(f"'{x}'" for x in sorted(KEEP_KIOSK))
    return f"""#!/bin/bash
set -euo pipefail
SRC="$1"
DEST_REPO="$2"
DEST_KIOSK="$3"
KEEP_REPO=({keep_repo})
KEEP_KIOSK=({keep_kiosk})

should_skip() {{
  local rel="$1"
  shift
  local -a keep=("$@")
  for k in "${{keep[@]}}"; do
    if [[ "$rel" == "$k" || "$rel" == "$k/"* ]]; then
      return 0
    fi
  done
  return 1
}}

overlay() {{
  local src="$1" dest="$2"
  shift 2
  local -a keep=("$@")
  mkdir -p "$dest"
  while IFS= read -r -d '' path; do
    rel="${{path#$src/}}"
    [[ -z "$rel" ]] && continue
    if should_skip "$rel" "${{keep[@]}}"; then
      continue
    fi
    if [[ "$path" == */__pycache__/* || "$path" == */__pycache__ ]]; then
      continue
    fi
    target="$dest/$rel"
    if [[ -d "$path" ]]; then
      mkdir -p "$target"
    else
      mkdir -p "$(dirname "$target")"
      cp -a "$path" "$target"
    fi
  done < <(find "$src" -mindepth 1 -print0)
}}

overlay "$SRC" "$DEST_REPO" "${{KEEP_REPO[@]}}"
overlay "$SRC/apps/pidi" "$DEST_KIOSK" "${{KEEP_KIOSK[@]}}"
echo "overlay complete"
"""


def version_payload(sha: str, branch: str) -> dict[str, object]:
    components: dict[str, str] = {
        "engines": sha256_file(STAGE / "pidi-native")[:16],
    }
    ui = ROOT / "apps" / "pidi" / "midi_tone.py"
    if ui.is_file():
        components["ui"] = sha256_file(ui)[:16]
    return {
        "sha": sha,
        "branch": branch,
        "source": "ssh-deploy",
        "repo_url": "local",
        "updated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "components": components,
    }


def deploy(*, branch: str | None, skip_bins: bool, restart: bool) -> int:
    for name in ENGINE_BINS:
        if not skip_bins and not (STAGE / name).is_file():
            print(f"missing {STAGE / name}", file=sys.stderr)
            return 1

    sha = git_sha()
    branch_name = branch or git_branch()
    print(f"deploying {branch_name} @ {sha[:7]} …", flush=True)

    tar_data = build_tar()
    print(
        f"repo tar: {len(tar_data) // 1024} KiB sha256={sha256_bytes(tar_data)[:16]}…",
        flush=True,
    )

    creds = load_creds()
    remote_repo, remote_kiosk, data_root = remote_paths(creds)
    password = creds["PI_PASSWORD"]
    client = connect(creds)
    stamp = int(time.time())
    slug = branch_name.replace("/", "-")
    remote_tar = f"/tmp/pi-deploy-{slug}-{stamp}.tar.gz"
    remote_src = f"/tmp/pi-deploy-src-{stamp}"
    overlay_path = f"/tmp/pi-overlay-{stamp}.sh"

    try:
        run(
            client,
            f"mkdir -p {remote_repo}/bin {remote_kiosk}/bin "
            f"{data_root}/{{songs,phrases,user-presets,user-wavetables,takes}}",
        )
        sftp_put_bytes(client, tar_data, remote_tar)
        run(
            client,
            f"rm -rf '{remote_src}' && mkdir -p '{remote_src}' && "
            f"tar -xzf '{remote_tar}' -C '{remote_src}'",
        )
        sftp_put_bytes(client, overlay_script().encode(), overlay_path)
        run(
            client,
            f"chmod +x '{overlay_path}' && bash '{overlay_path}' "
            f"'{remote_src}' '{remote_repo}' '{remote_kiosk}'",
        )
        run(
            client,
            f"find {remote_repo}/apps/pidi -name '*.sh' -type f -print0 2>/dev/null | "
            f"xargs -0 sed -i 's/\\r$//' || true; "
            f"find {remote_kiosk} -name '*.sh' -type f -print0 2>/dev/null | "
            f"xargs -0 sed -i 's/\\r$//' || true",
        )

        if not skip_bins:
            sudo(client, password, "systemctl stop pidi-native jambox-engine 2>/dev/null || true")
            for name in ENGINE_BINS:
                sftp_put_file(client, STAGE / name, f"{remote_repo}/bin/{name}")
                sftp_put_file(client, STAGE / name, f"{remote_kiosk}/bin/{name}")

        version_json = json.dumps(version_payload(sha, branch_name), indent=2) + "\n"
        for dest in (f"{remote_kiosk}/version.json", f"{remote_repo}/apps/pidi/version.json"):
            sftp_put_bytes(client, version_json.encode(), dest + ".tmp")
            run(client, f"mv -f '{dest}.tmp' '{dest}'")

        if restart and not skip_bins:
            sudo(client, password, "systemctl daemon-reload")
            sudo(client, password, "systemctl enable jambox-engine pidi-native")
            sudo(client, password, "systemctl restart jambox-engine")
            time.sleep(2)
            sudo(client, password, "systemctl restart pidi-native")

        run(
            client,
            f"ls -la {remote_repo}/bin/; "
            f"head -8 {remote_kiosk}/version.json; "
            f"systemctl is-active jambox-engine pidi-native",
        )
        if restart and not skip_bins:
            run(client, "journalctl -u jambox-engine -u pidi-native -n 20 --no-pager 2>&1")
        run(client, f"rm -rf '{remote_src}' '{remote_tar}' '{overlay_path}'")
    finally:
        client.close()

    print("\nDeploy complete.", flush=True)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Deploy current branch to the lab Pi")
    parser.add_argument(
        "--branch",
        help="Branch name to record in version.json (default: current git branch)",
    )
    parser.add_argument(
        "--skip-bins",
        action="store_true",
        help="Overlay source only; do not upload dist/armv7 engines",
    )
    parser.add_argument(
        "--no-restart",
        action="store_true",
        help="Upload without restarting jambox-engine / pidi-native",
    )
    args = parser.parse_args(argv)
    return deploy(branch=args.branch, skip_bins=args.skip_bins, restart=not args.no_restart)


if __name__ == "__main__":
    raise SystemExit(main())
