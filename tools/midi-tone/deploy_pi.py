#!/usr/bin/env python3
"""Deploy midi-tone to the Pi using .pi-credentials (gitignored)."""

from __future__ import annotations

import argparse
import os
import pathlib
import sys
import time

try:
    import paramiko
except ImportError:
    sys.exit("pip install paramiko")


HERE = pathlib.Path(__file__).resolve().parent
CREDS = HERE / ".pi-credentials"

FILES = [
    "midi_tone.py",
    "requirements.txt",
    "run.sh",
    "setup-venv.sh",
    "install-desktop-shortcut.sh",
    "midi-tone.desktop",
    "README.md",
    "fix-audio-headphones.sh",
]


def load_creds() -> dict[str, str]:
    if not CREDS.exists():
        sys.exit(f"Missing {CREDS}")
    out: dict[str, str] = {}
    for line in CREDS.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        out[k.strip()] = v.strip()
    for key in ("PI_HOST", "PI_USER", "PI_PASSWORD"):
        if key not in out:
            sys.exit(f"Missing {key} in .pi-credentials")
    return out


def connect(creds: dict[str, str]) -> paramiko.SSHClient:
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(
        creds["PI_HOST"],
        username=creds["PI_USER"],
        password=creds["PI_PASSWORD"],
        timeout=15,
        allow_agent=False,
        look_for_keys=False,
    )
    return client


def run(
    client: paramiko.SSHClient,
    cmd: str,
    timeout: int = 120,
    check: bool = True,
) -> str:
    print(f"$ {cmd}")
    _stdin, stdout, stderr = client.exec_command(cmd, timeout=timeout, get_pty=False)
    out = stdout.read().decode("utf-8", errors="replace")
    err = stderr.read().decode("utf-8", errors="replace")
    code = stdout.channel.recv_exit_status()
    if out.strip():
        print(out.rstrip().encode("ascii", "replace").decode("ascii"))
    if err.strip():
        print(err.rstrip().encode("ascii", "replace").decode("ascii"), file=sys.stderr)
    if check and code != 0:
        raise SystemExit(f"Remote command failed ({code}): {cmd}")
    return out


def deploy(restart: bool) -> None:
    creds = load_creds()
    remote_dir = creds.get("PI_DIR", "~/midi-tone").replace("~", f"/home/{creds['PI_USER']}")
    client = connect(creds)
    try:
        run(client, f"mkdir -p {remote_dir}")
        sftp = client.open_sftp()
        try:
            for name in FILES:
                local = HERE / name
                if not local.exists():
                    print(f"skip missing {name}")
                    continue
                remote = f"{remote_dir}/{name}"
                print(f"put {name} -> {remote}")
                sftp.put(str(local), remote)
        finally:
            sftp.close()

        run(
            client,
            f"sed -i 's/\\r$//' {remote_dir}/*.sh {remote_dir}/*.desktop 2>/dev/null; "
            f"chmod +x {remote_dir}/*.sh",
        )
        # Refresh menu/desktop launchers so they keep using the venv via run.sh
        run(client, f"bash {remote_dir}/install-desktop-shortcut.sh", check=False)

        if restart:
            run(client, "pkill -f '[m]idi_tone.py' || true", check=False)
            time.sleep(0.5)
            start = (
                f"bash -lc 'export DISPLAY=:0 XDG_RUNTIME_DIR=/run/user/$(id -u); "
                f"cd {remote_dir}; "
                f"nohup ./run.sh --input MPK >/tmp/midi-tone.log 2>&1 & echo $!'"
            )
            run(client, start)
            time.sleep(1.5)
            run(client, "tail -n 40 /tmp/midi-tone.log || true", check=False)
        print("Deploy OK")
    finally:
        client.close()


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--restart", action="store_true")
    args = p.parse_args()
    deploy(restart=args.restart)


if __name__ == "__main__":
    main()
