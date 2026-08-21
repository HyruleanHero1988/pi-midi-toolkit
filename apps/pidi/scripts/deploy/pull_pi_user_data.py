#!/usr/bin/env python3
"""Copy PiDI user data from the Pi into apps/pidi/_pi-user-backup/<stamp>/."""
from __future__ import annotations

import datetime
import io
import pathlib
import tarfile

import paramiko

HERE = pathlib.Path(__file__).resolve().parents[2]
PULL = (
    "phrases",
    "songs",
    "user-presets",
    "user-wavetables",
    "settings.json",
    "version.json",
    "takes",
)


def main() -> None:
    creds: dict[str, str] = {}
    for line in (HERE / ".pi-credentials").read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        creds[k.strip()] = v.strip()

    remote_dir = creds.get("PI_DIR", "~/midi-tone").replace(
        "~", f"/home/{creds['PI_USER']}"
    )
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    dest = HERE / "_pi-user-backup" / stamp
    dest.mkdir(parents=True, exist_ok=True)

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
    try:
        names = " ".join(PULL)
        _, stdout, _ = client.exec_command(
            f"cd {remote_dir} && for p in {names}; do "
            f'test -e "$p" && printf "%s\\n" "$p"; done'
        )
        existing = [
            ln.strip() for ln in stdout.read().decode("utf-8").splitlines() if ln.strip()
        ]
        print("pulling:", existing, flush=True)
        if not existing:
            raise SystemExit("nothing to pull")

        tar_cmd = f"cd {remote_dir} && tar -czf - " + " ".join(existing)
        _, stdout, stderr = client.exec_command(tar_cmd)
        data = stdout.read()
        err = stderr.read().decode("utf-8", errors="replace")
        code = stdout.channel.recv_exit_status()
        if code != 0:
            raise SystemExit(f"tar failed ({code}): {err}")
    finally:
        client.close()

    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tf:
        try:
            tf.extractall(dest, filter="data")
        except TypeError:
            tf.extractall(dest)

    files = sorted(p for p in dest.rglob("*") if p.is_file())
    print(f"\nSaved {len(files)} files → {dest}", flush=True)
    for p in files:
        print(f"  {p.relative_to(dest)} ({p.stat().st_size} bytes)", flush=True)


if __name__ == "__main__":
    main()
