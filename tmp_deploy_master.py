#!/usr/bin/env python3
"""Deploy local master tree to the lab Pi (OTA-style overlay + native bins)."""

from __future__ import annotations

import hashlib
import io
import json
import pathlib
import subprocess
import sys
import tarfile
import time

import paramiko

ROOT = pathlib.Path(__file__).resolve().parent
CREDS_PATH = ROOT / "apps" / "pidi" / ".pi-credentials"
STAGE = ROOT / "dist" / "armv7"
REMOTE_REPO = "/home/ray/pi-midi-toolkit"
REMOTE_KIOSK = "/home/ray/midi-tone"
DATA = "/home/ray/.local/share/pidi"

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


def load_creds() -> dict[str, str]:
    creds: dict[str, str] = {}
    for line in CREDS_PATH.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        creds[k.strip()] = v.strip()
    return creds


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


def should_skip(rel: pathlib.Path) -> bool:
    parts = rel.parts
    if not parts:
        return True
    if any(p in TAR_SKIP_PARTS for p in parts):
        return True
    if rel.suffix == ".pyc":
        return True
    return False


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


def sftp_put_file(client: paramiko.SSHClient, local: pathlib.Path, remote: str, *, mode: int = 0o755) -> None:
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


def main() -> int:
    if not CREDS_PATH.is_file():
        print(f"missing {CREDS_PATH}", file=sys.stderr)
        return 1
    for name in ("jambox-engine", "pidi-native", "midi-engine"):
        if not (STAGE / name).is_file():
            print(f"missing {STAGE / name}", file=sys.stderr)
            return 1

    sha = git_sha()
    print(f"deploying master {sha[:7]} …", flush=True)

    tar_data = build_tar()
    print(f"repo tar: {len(tar_data) // 1024} KiB sha256={sha256_bytes(tar_data)[:16]}…", flush=True)

    creds = load_creds()
    password = creds["PI_PASSWORD"]
    client = connect(creds)
    stamp = int(time.time())
    remote_tar = f"/tmp/pi-master-{stamp}.tar.gz"
    remote_src = f"/tmp/pi-master-src-{stamp}"
    try:
        run(
            client,
            f"mkdir -p {REMOTE_REPO}/bin {REMOTE_KIOSK}/bin "
            f"{DATA}/{{songs,phrases,user-presets,user-wavetables,takes}}",
        )
        sftp_put_bytes(client, tar_data, remote_tar)
        run(client, f"rm -rf '{remote_src}' && mkdir -p '{remote_src}' && tar -xzf '{remote_tar}' -C '{remote_src}'")

        overlay_path = f"/tmp/pi-overlay-{stamp}.sh"
        sftp_put_bytes(client, overlay_script().encode(), overlay_path)
        run(client, f"chmod +x '{overlay_path}' && bash '{overlay_path}' '{remote_src}' '{REMOTE_REPO}' '{REMOTE_KIOSK}'")

        run(
            client,
            f"find {REMOTE_REPO}/apps/pidi -name '*.sh' -type f -print0 2>/dev/null | xargs -0 sed -i 's/\\r$//' || true; "
            f"find {REMOTE_KIOSK} -name '*.sh' -type f -print0 2>/dev/null | xargs -0 sed -i 's/\\r$//' || true",
        )

        sudo(client, password, "systemctl stop pidi-native jambox-engine 2>/dev/null || true")
        for name in ("jambox-engine", "pidi-native", "midi-engine"):
            sftp_put_file(client, STAGE / name, f"{REMOTE_REPO}/bin/{name}")
            sftp_put_file(client, STAGE / name, f"{REMOTE_KIOSK}/bin/{name}")

        version = {
            "sha": sha,
            "branch": "master",
            "source": "ssh-deploy",
            "repo_url": "local",
            "updated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "components": {
                "ui": sha256_file(ROOT / "apps" / "pidi" / "midi_tone.py")[:16],
                "engines": sha256_file(STAGE / "pidi-native")[:16],
            },
        }
        version_json = json.dumps(version, indent=2) + "\n"
        for dest in (f"{REMOTE_KIOSK}/version.json", f"{REMOTE_REPO}/apps/pidi/version.json"):
            sftp_put_bytes(client, version_json.encode(), dest + ".tmp")
            run(client, f"mv -f '{dest}.tmp' '{dest}'")

        unit_path = ROOT / "tmp_pidi-native.service"
        if unit_path.is_file():
            sftp_put_bytes(client, unit_path.read_bytes(), "/tmp/pidi-native.service")
            sudo(
                client,
                password,
                "cp /tmp/pidi-native.service /etc/systemd/system/pidi-native.service",
            )

        sudo(client, password, "systemctl daemon-reload")
        sudo(client, password, "systemctl enable jambox-engine pidi-native")
        sudo(client, password, "systemctl restart jambox-engine")
        time.sleep(2)
        sudo(client, password, "systemctl restart pidi-native")

        run(
            client,
            f"ls -la {REMOTE_REPO}/bin/; "
            f"head -5 {REMOTE_KIOSK}/version.json; "
            f"systemctl is-active jambox-engine pidi-native",
        )
        run(client, "journalctl -u jambox-engine -u pidi-native -n 20 --no-pager 2>&1")
        run(client, f"rm -rf '{remote_src}' '{remote_tar}' '{overlay_path}'")
    finally:
        client.close()

    print("\nDeploy complete.", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
