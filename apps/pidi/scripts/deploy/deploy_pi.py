#!/usr/bin/env python3
"""Deploy PiDI to the Pi using .pi-credentials (gitignored).

Run from anywhere::

    python apps/pidi/scripts/deploy/deploy_pi.py --restart
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys
import time

try:
    import paramiko
except ImportError:
    sys.exit("pip install paramiko")


HERE = pathlib.Path(__file__).resolve().parents[2]  # apps/pidi deploy root
CREDS = HERE / ".pi-credentials"

FILES = [
    "midi_tone.py",
    "kiosk.sh",
    "run.sh",
    "launch-desktop.sh",
    "install-kiosk.sh",
    "disable-kiosk.sh",
    "setup-venv.sh",
    "midi-tone.desktop",
    "README.md",
    "KIOSK.md",
    "ARCHITECTURE.md",
    "requirements.txt",
]

DIRS = [
    "bin",
    "scripts",
    "pidi",
    "tests",
    "wavetables",
    "kiosk",
    "demo-songs",
    "branding",
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
        timeout=60,
        banner_timeout=60,
        auth_timeout=60,
        allow_agent=False,
        look_for_keys=False,
    )
    return client


def run(
    client: paramiko.SSHClient,
    cmd: str,
    *,
    check: bool = True,
    timeout: int = 120,
) -> str:
    print(f"$ {cmd}", flush=True)
    _stdin, stdout, stderr = client.exec_command(cmd, timeout=timeout)
    out = stdout.read().decode("utf-8", errors="replace")
    err = stderr.read().decode("utf-8", errors="replace")
    code = stdout.channel.recv_exit_status()
    if out.strip():
        print(out.rstrip(), flush=True)
    if err.strip():
        print(err.rstrip(), file=sys.stderr, flush=True)
    if check and code != 0:
        raise RuntimeError(f"remote exit {code}: {cmd}")
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description="Deploy PiDI to the Pi")
    parser.add_argument("--restart", action="store_true", help="Restart kiosk after upload")
    args = parser.parse_args()

    creds = load_creds()
    remote_dir = creds.get("PI_DIR", "~/midi-tone").replace("~", f"/home/{creds['PI_USER']}")
    client = connect(creds)
    try:
        run(client, f"mkdir -p {remote_dir}")
        sftp = client.open_sftp()
        try:
            for name in FILES:
                local = HERE / name
                if not local.is_file():
                    print(f"skip missing {name}", flush=True)
                    continue
                remote = f"{remote_dir}/{name}"
                print(f"put {name}", flush=True)
                sftp.put(str(local), remote)
            for name in DIRS:
                local = HERE / name
                if not local.is_dir():
                    print(f"skip missing dir {name}", flush=True)
                    continue
                # Recursive upload via tar over SSH is simpler than walking SFTP.
                print(f"sync {name}/", flush=True)
        finally:
            sftp.close()

        # Bulk-copy trees with tar (preserves modes better than naive SFTP walk).
        for name in DIRS:
            local = HERE / name
            if not local.is_dir():
                continue
            tar = subprocess.run(
                ["tar", "-C", str(HERE), "-cf", "-", name],
                check=True,
                capture_output=True,
            )
            cmd = f"tar -C {remote_dir} -xf -"
            stdin, stdout, stderr = client.exec_command(cmd)
            stdin.write(tar.stdout)
            stdin.channel.shutdown_write()
            _ = stdout.read()
            err = stderr.read().decode("utf-8", errors="replace")
            code = stdout.channel.recv_exit_status()
            if code != 0:
                raise RuntimeError(f"tar upload {name} failed: {err}")

        run(
            client,
            f"cd {remote_dir} && find bin scripts -name '*.sh' -exec chmod +x {{}} + "
            f"&& chmod +x kiosk.sh run.sh launch-desktop.sh install-kiosk.sh "
            f"disable-kiosk.sh setup-venv.sh 2>/dev/null || true",
            check=False,
        )
        run(client, f"bash {remote_dir}/scripts/install/install-desktop-shortcut.sh", check=False)

        if args.restart:
            run(
                client,
                "pgrep -af 'kiosk\\.sh|midi-tone-kiosk|python -m pidi|midi_tone' || true",
                check=False,
            )
            run(
                client,
                "pkill -f 'python -m pidi' || true; pkill -f midi_tone || true",
                check=False,
            )
            time.sleep(1)
            run(
                client,
                f"bash -lc 'cd {remote_dir} && export DISPLAY=:0 "
                f"XDG_RUNTIME_DIR=/run/user/$(id -u) && ./launch-desktop.sh'",
                check=False,
            )
            run(client, "tail -n 40 /tmp/midi-tone.log || true", check=False)
            run(client, "tail -n 20 /tmp/midi-tone-kiosk.log || true", check=False)
    finally:
        client.close()
    print("deploy done", flush=True)


if __name__ == "__main__":
    main()
